//! Who the engine decides a connection is, established from the kernel.
//!
//! Every primitive reaches the engine over the same Unix socket, and until
//! acton-reactive 9.4.0 the only name on a message was the `source` field the
//! client wrote itself. This module answers the other question: which process
//! opened this connection, and is it one the engine started?
//!
//! # How a peer becomes a primitive
//!
//! The kernel reports the peer's pid at accept time. The engine knows the pid
//! of every child it spawned. Those are often the same process, and then the
//! answer is one hash lookup. They are not the same when the configured `path`
//! is a launcher: `uv run python -m app` leaves `uv` as the engine's child and
//! the interpreter, a grandchild, is what opens the socket. So
//! [`resolve_ancestry`] walks up `/proc/<pid>/stat` from the peer until it
//! reaches a pid the engine recorded, and gives up at the engine itself, at pid
//! 1, at the first parent the lookup cannot supply, or after
//! [`MAX_ANCESTRY_DEPTH`] steps.
//!
//! [`resolve_ancestry`] is pure: the parent lookup and the child table are both
//! arguments. [`proc_parent_of`] is the real reader and the only part that
//! touches the filesystem.
//!
//! # Why a recycled pid cannot become a primitive
//!
//! acton says of the pid it hands a policy that "a PID can be recycled;
//! applications using PID-based admission must manage the process lifetime"
//! (`security.rs:74-75`). The engine does manage it, and that is what makes
//! the lookup sound rather than merely likely:
//!
//! - A pid in the child table belongs to a child the engine has spawned and
//!   not yet reaped. Until the parent reaps it the kernel keeps the pid as a
//!   zombie, so it cannot be handed to a new process. The table entry is
//!   cleared in the same task step in which `Child::wait` returns, with no
//!   await between the reap and the clear (the monitor task in
//!   [`crate::primitive_actor`]), so there is no window in which a reaped pid
//!   is still in the table.
//! - A pid the walk merely passes through, a `uv` launcher's interpreter say,
//!   is not held by the engine and can be recycled once its own chain dies.
//!   That costs nothing: the walk from a recycled pid climbs the new process's
//!   own ancestry, which no longer reaches a live recorded child, so it
//!   resolves as [`Resolution::Foreign`]. The only way the walk reaches a
//!   recorded child is if the peer really is that child's descendant.
//!
//! So a pid is a primitive exactly while the engine holds that child as live,
//! which is the same window in which revocation is meaningful. The decision is
//! [`resolve_ancestry`], and the case where the table no longer holds the child
//! is tested there.
//!
//! # Where there is no `/proc`
//!
//! [`proc_parent_of`] reports no parent on a platform without `/proc`, which
//! collapses the walk to a direct pid match. A primitive the engine exec'd
//! itself still resolves by name; one behind a launcher such as `uv` resolves
//! as [`ConnectionIdentity::Unmanaged`]. [`ancestry_available`] is how the rest
//! of the engine finds out, so that enforcement can keep the weaker
//! self-reported-name behaviour there instead of refusing honest primitives.
//!
//! # Revocation
//!
//! An identity outlives nothing: when a primitive's child exits, every
//! connection admitted under its name is revoked. [`ConnectionRegistry`] keeps
//! the map that needs, fed by
//! [`crate::ipc_policy::PolicyObserver::on_admitted`] and drained by
//! [`ChildExitObserver::on_child_exit`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use acton_reactive::ipc::{IpcAccessDenied, IpcListenerHandle, PeerCredentials};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Who the engine decided a connection is.
///
/// Stored in acton's `IpcIdentity` at admission and read back in `authorize`
/// with `ctx.identity::<ConnectionIdentity>()`. It comes from the kernel and
/// the engine's own process table, never from a client payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionIdentity {
    /// A process the engine spawned (or a descendant of one), by primitive name.
    Primitive {
        /// The name the primitive is configured under.
        name: String,
    },
    /// A same-user process the engine did not spawn: CLI queries, a developer
    /// running an SDK program by hand.
    Unmanaged {
        /// The peer's pid, when the platform reported one.
        pid: Option<u32>,
        /// The peer's user id, or `0` where the platform reported no
        /// credentials at all. Admission reads the kernel's answer directly
        /// rather than this field, so the placeholder never decides anything.
        uid: u32,
    },
}

impl ConnectionIdentity {
    /// The primitive name, when the engine established one.
    #[must_use]
    pub fn primitive_name(&self) -> Option<&str> {
        match self {
            Self::Primitive { name } => Some(name),
            Self::Unmanaged { .. } => None,
        }
    }

    /// Whether the engine vouches for this connection's name.
    #[must_use]
    pub const fn is_managed(&self) -> bool {
        matches!(self, Self::Primitive { .. })
    }
}

impl std::fmt::Display for ConnectionIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Primitive { name } => f.write_str(name),
            Self::Unmanaged { pid: Some(pid), .. } => write!(f, "<unmanaged pid {pid}>"),
            Self::Unmanaged { pid: None, .. } => f.write_str("<unmanaged>"),
        }
    }
}

/// Build the identity for a peer the engine did not spawn (pure function).
///
/// Split out because `PeerCredentials` has no public constructor, so the
/// mapping can only be tested through the values read out of it.
#[must_use]
pub const fn unmanaged_from(pid: Option<u32>, uid: u32) -> ConnectionIdentity {
    ConnectionIdentity::Unmanaged { pid, uid }
}

// ---------------------------------------------------------------------------
// What the engine does about a peer it did not spawn
// ---------------------------------------------------------------------------

/// How the engine treats a connection it cannot tie to a primitive it spawned.
///
/// Resolving the identity happens in every mode, because declaration
/// enforcement and startup readiness both need a trustworthy name. This only
/// decides whether an unmanaged peer is let in.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthenticationMode {
    /// Any peer that can open the socket is admitted, as every engine up to
    /// 0.10.10 did. The identity is still established.
    #[default]
    Off,
    /// Unmanaged peers are admitted and logged at WARN with their pid and uid.
    Warn,
    /// A peer whose user id is not the engine's own, or whose credentials the
    /// platform declined to report, is refused. A same-user unmanaged peer is
    /// still admitted: see [`admission_for`] for why.
    Strict,
}

impl AuthenticationMode {
    /// The name this mode is written under in `[engine].authenticate_connections`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Warn => "warn",
            Self::Strict => "strict",
        }
    }

    /// Whether the engine has anything to say about unmanaged peers.
    #[must_use]
    pub const fn is_enforcing(&self) -> bool {
        !matches!(self, Self::Off)
    }
}

impl std::fmt::Display for AuthenticationMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What admission does with a resolved identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionDecision {
    /// Let the connection in without comment.
    Admit,
    /// Let it in, and say in the log that the engine did not spawn it.
    AdmitUnmanaged,
    /// Refuse it, with this sentence.
    Deny(String),
}

impl AdmissionDecision {
    /// Whether the connection is let in.
    #[must_use]
    pub const fn is_admitted(&self) -> bool {
        !matches!(self, Self::Deny(_))
    }
}

/// Decide whether a resolved peer is let in (pure function).
///
/// A primitive the engine spawned is always admitted. What happens to anything
/// else is the whole of `[engine].authenticate_connections`:
///
/// `strict` refuses a peer from another user id, which the socket's own `0o660`
/// mode does not: a second account in the engine's group can open it. It does
/// not refuse a same-user unmanaged peer, because the engine cannot tell the
/// `emergent` CLI, a primitive run by hand and a developer's own SDK program
/// apart from an impostor, and refusing all four would take the working tools
/// with the attack. What closes that gap is not admission but attribution: an
/// unmanaged connection cannot borrow a configured primitive's name once
/// [`crate::ipc_policy`] has a trusted one to compare against.
#[must_use]
pub fn admission_for(
    mode: AuthenticationMode,
    identity: &ConnectionIdentity,
    peer_uid: Option<u32>,
    engine_uid: u32,
) -> AdmissionDecision {
    if identity.is_managed() {
        return AdmissionDecision::Admit;
    }
    match mode {
        AuthenticationMode::Off => AdmissionDecision::Admit,
        AuthenticationMode::Warn => AdmissionDecision::AdmitUnmanaged,
        AuthenticationMode::Strict => match peer_uid {
            Some(uid) if uid == engine_uid => AdmissionDecision::AdmitUnmanaged,
            Some(uid) => AdmissionDecision::Deny(format!(
                "connections are restricted to uid {engine_uid} and this peer is uid {uid}"
            )),
            None => AdmissionDecision::Deny(
                "connections are restricted by user id and this platform reported no peer credentials"
                    .to_owned(),
            ),
        },
    }
}

// ---------------------------------------------------------------------------
// The ancestry walk
// ---------------------------------------------------------------------------

/// How far the ancestry walk climbs before giving up.
///
/// A launcher chain is one or two processes deep in practice (`uv` to
/// `python3`, or a shell to a binary). The bound is what keeps a `/proc` that
/// reports a cycle, or a pid recycled mid-walk, from spinning.
pub const MAX_ANCESTRY_DEPTH: usize = 16;

/// What the ancestry walk found, which is more than just the identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The connection belongs to this primitive.
    Primitive(String),
    /// The walk reached the engine without passing a pid the engine has
    /// recorded, so the peer is a descendant of the engine that the child
    /// table does not name yet. Another look shortly is worth taking.
    UnrecordedChild,
    /// The walk left the engine's process tree, or could not start. Nothing
    /// about this peer will change by looking again.
    Foreign,
}

/// Tie a connecting process to the primitive the engine spawned for it (pure).
///
/// Walks from `peer_pid` up through parents, returning the first pid found in
/// `children`. Stops at the engine itself, at pid 1, at the first parent the
/// lookup cannot supply, and after [`MAX_ANCESTRY_DEPTH`] steps.
#[must_use]
pub fn resolve_ancestry(
    peer_pid: Option<u32>,
    engine_pid: u32,
    children: &HashMap<u32, String>,
    parent_of: &dyn Fn(u32) -> Option<u32>,
) -> Resolution {
    let Some(mut pid) = peer_pid else {
        return Resolution::Foreign;
    };

    for _ in 0..MAX_ANCESTRY_DEPTH {
        if let Some(name) = children.get(&pid) {
            return Resolution::Primitive(name.clone());
        }
        if pid == engine_pid {
            return Resolution::UnrecordedChild;
        }
        if pid <= 1 {
            return Resolution::Foreign;
        }
        match parent_of(pid) {
            Some(parent) => pid = parent,
            None => return Resolution::Foreign,
        }
    }
    Resolution::Foreign
}

/// Read a process's parent from `/proc`, where `/proc` exists.
///
/// On Linux this is `/proc/<pid>/stat` field 4. Everywhere else it reports no
/// parent, which collapses the walk to a direct pid match. See the module docs
/// for what that costs.
#[must_use]
pub fn proc_parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Field 2 is the executable name in parentheses and may itself contain
    // spaces and parentheses, so the fields after it are counted from the LAST
    // ')'. Splitting from the start would read part of the name as the ppid.
    let after_comm = stat.rsplit_once(')')?.1;
    // After the name come: state, ppid, ...
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

/// Whether this platform can tell the engine a process's parent.
///
/// Asked once at startup. Where it answers `false`, a primitive launched
/// behind a wrapper arrives unmanaged and the engine has to say so rather than
/// refuse it.
#[must_use]
pub fn ancestry_available() -> bool {
    proc_parent_of(std::process::id()).is_some()
}

// ---------------------------------------------------------------------------
// The resolver
// ---------------------------------------------------------------------------

/// Resolves a connecting peer to an identity.
///
/// `admit` awaits this, so it may read shared engine state.
///
/// # Errors
///
/// Returns a denial to refuse the connection outright. A resolver that cannot
/// name a peer answers [`ConnectionIdentity::Unmanaged`] rather than denying,
/// and leaves the consequences to the configured modes.
#[async_trait::async_trait]
pub trait IdentityResolver: Send + Sync + std::panic::RefUnwindSafe + 'static {
    /// Decide who a connecting peer is.
    async fn resolve(
        &self,
        peer: Option<PeerCredentials>,
    ) -> Result<ConnectionIdentity, IpcAccessDenied>;
}

/// A live view of the children the engine has spawned, by pid.
///
/// Admission is the only asynchronous hook acton offers, which is why the
/// identity is resolved once there and then carried on the connection for its
/// lifetime. The process manager implements this over the same watch channels
/// startup readiness joins against, so there is one table and not two.
#[async_trait::async_trait]
pub trait SpawnedChildren: Send + Sync + 'static {
    /// The pid of every primitive the engine currently holds as running.
    async fn child_names_by_pid(&self) -> HashMap<u32, String>;
}

/// How many times admission re-reads the child table before giving up.
const DEFAULT_ADMISSION_ATTEMPTS: u32 = 5;
/// How long admission waits between those reads.
const DEFAULT_ADMISSION_BACKOFF: Duration = Duration::from_millis(40);

/// Names a peer by walking its process ancestry to a child the engine spawned.
pub struct AncestryResolver {
    children: Arc<dyn SpawnedChildren>,
    mode: AuthenticationMode,
    engine_pid: u32,
    engine_uid: u32,
    attempts: u32,
    backoff: Duration,
}

// The resolver's own state is fixed once it is built and `resolve` takes
// `&self`, so nothing it owns can be left half written by a panic. The engine
// state reached through `children` is read, never modified, and its
// consistency is the process manager's business.
impl std::panic::RefUnwindSafe for AncestryResolver {}

impl AncestryResolver {
    /// Build a resolver over the engine's live child table.
    #[must_use]
    pub fn new(children: Arc<dyn SpawnedChildren>, mode: AuthenticationMode) -> Self {
        Self {
            children,
            mode,
            engine_pid: std::process::id(),
            engine_uid: nix::unistd::Uid::effective().as_raw(),
            attempts: DEFAULT_ADMISSION_ATTEMPTS,
            backoff: DEFAULT_ADMISSION_BACKOFF,
        }
    }

    /// Tell the resolver who the engine is. Tests use this; startup does not.
    #[must_use]
    pub const fn with_process_facts(mut self, engine_pid: u32, engine_uid: u32) -> Self {
        self.engine_pid = engine_pid;
        self.engine_uid = engine_uid;
        self
    }

    /// Override the retry budget.
    #[must_use]
    pub const fn with_admission_retry(mut self, attempts: u32, backoff: Duration) -> Self {
        self.attempts = attempts;
        self.backoff = backoff;
        self
    }

    /// Walk the peer's ancestry, waiting briefly for a pid the engine has
    /// spawned but not yet recorded.
    ///
    /// The engine records a child's pid on its own task, so a fast primitive
    /// can connect before the table names it. Only a peer inside the engine's
    /// process tree can be in that position, which is exactly
    /// [`Resolution::UnrecordedChild`]; anything else is answered on the first
    /// look, so a CLI query pays nothing for the retry.
    async fn walk(&self, peer_pid: Option<u32>) -> Resolution {
        for attempt in 1..=self.attempts {
            let children = self.children.child_names_by_pid().await;
            let resolution =
                resolve_ancestry(peer_pid, self.engine_pid, &children, &proc_parent_of);
            if resolution != Resolution::UnrecordedChild || attempt == self.attempts {
                if attempt > 1 {
                    debug!(
                        attempt,
                        peer.pid = ?peer_pid,
                        "Resolved a peer the child table did not name at first look"
                    );
                }
                return resolution;
            }
            tokio::time::sleep(self.backoff).await;
        }
        Resolution::Foreign
    }
}

#[async_trait::async_trait]
impl IdentityResolver for AncestryResolver {
    async fn resolve(
        &self,
        peer: Option<PeerCredentials>,
    ) -> Result<ConnectionIdentity, IpcAccessDenied> {
        let peer_pid = peer.and_then(PeerCredentials::pid);
        let peer_uid = peer.map(PeerCredentials::uid);
        let identity = match self.walk(peer_pid).await {
            Resolution::Primitive(name) => ConnectionIdentity::Primitive { name },
            Resolution::UnrecordedChild | Resolution::Foreign => {
                unmanaged_from(peer_pid, peer_uid.unwrap_or(0))
            }
        };

        match admission_for(self.mode, &identity, peer_uid, self.engine_uid) {
            AdmissionDecision::Admit => Ok(identity),
            AdmissionDecision::AdmitUnmanaged => {
                warn!(
                    peer.pid = ?peer_pid,
                    peer.uid = ?peer_uid,
                    mode = %self.mode,
                    "Admitted a connection from a process the engine did not spawn"
                );
                Ok(identity)
            }
            AdmissionDecision::Deny(reason) => {
                warn!(
                    peer.pid = ?peer_pid,
                    peer.uid = ?peer_uid,
                    mode = %self.mode,
                    "Refused an IPC connection: {reason}"
                );
                Err(IpcAccessDenied::new(reason))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Revocation
// ---------------------------------------------------------------------------

/// Closes one admitted connection.
///
/// Implemented over acton's `IpcListenerHandle::revoke_connection`, which
/// "revokes a connection and closes its socket, cancelling pending IPC work"
/// (`listener.rs:421-425`). A trait rather than the handle itself, so the
/// registry's decisions can be tested without a listener.
pub trait ConnectionRevoker: Send + Sync + 'static {
    /// Close this connection. Returns whether it was still open.
    fn revoke(&self, connection_id: usize) -> bool;
}

impl ConnectionRevoker for IpcListenerHandle {
    fn revoke(&self, connection_id: usize) -> bool {
        self.revoke_connection(connection_id)
    }
}

/// Told when a primitive's child process has exited.
///
/// The process manager drives this off the same pid watch shutdown waits on,
/// so it fires the moment the monitor task clears the pid, before any restart
/// backoff has elapsed.
pub trait ChildExitObserver: Send + Sync + 'static {
    /// The named primitive's child, which had this pid, is gone.
    fn on_child_exit(&self, name: &str, pid: u32);
}

/// How many connection ids are kept per primitive.
///
/// One primitive holds a handful at a time: the SDKs open a throwaway
/// discovery connection alongside the one they keep, and a restart overlaps
/// two. acton-reactive 9.4.1 offers a policy no hook for a connection closing
/// (`IpcSecurityPolicy` has exactly `admit` and `authorize`,
/// `security.rs:229-244`), so ids are forgotten when the primitive they belong
/// to exits rather than when its socket does, and this cap is what bounds the
/// set in between. Reaching it drops the oldest id, which is the one most
/// likely to be closed already.
pub const MAX_CONNECTIONS_PER_PRIMITIVE: usize = 64;

/// Add a connection id to one primitive's set (pure function).
///
/// Returns the id evicted to stay under `cap`, if any. An id already held
/// changes nothing: acton hands out a fresh id per connection, so a repeat can
/// only be a replay.
pub fn remember(ids: &mut Vec<usize>, connection_id: usize, cap: usize) -> Option<usize> {
    if ids.contains(&connection_id) {
        return None;
    }
    let evicted = (cap > 0 && ids.len() >= cap).then(|| ids.remove(0));
    ids.push(connection_id);
    evicted
}

/// Which connections a primitive holds, so they can be closed when it dies.
///
/// Built from [`crate::ipc_policy::PolicyObserver::on_admitted`], which acton
/// calls once per connection with the id it assigned. Only a connection the
/// engine could name is kept: an unmanaged peer has no child whose exit could
/// revoke it.
pub struct ConnectionRegistry {
    connections: Mutex<HashMap<String, Vec<usize>>>,
    revoker: OnceLock<Arc<dyn ConnectionRevoker>>,
}

// The registry's own state is a mutex over plain data and a write-once slot.
// A panic cannot leave a half-written connection id behind, and the lock is
// taken through `PoisonError::into_inner` so a poisoned mutex stays usable.
impl std::panic::RefUnwindSafe for ConnectionRegistry {}

impl Default for ConnectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionRegistry {
    /// An empty registry with nowhere yet to send a revocation.
    #[must_use]
    pub fn new() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
            revoker: OnceLock::new(),
        }
    }

    /// Give the registry the listener it revokes through.
    ///
    /// Separate from construction because the registry has to exist before the
    /// listener: the policy that feeds it is an argument to starting one.
    /// Revocation before this is called closes nothing and says so.
    pub fn install_revoker(&self, revoker: Arc<dyn ConnectionRevoker>) {
        if self.revoker.set(revoker).is_err() {
            debug!("Connection revoker already installed");
        }
    }

    /// Record a connection the engine admitted as a primitive.
    ///
    /// Called from acton's connection task: one mutex acquisition, one vector
    /// push, no awaiting and no I/O.
    pub fn record(&self, identity: &ConnectionIdentity, connection_id: usize) {
        let Some(name) = identity.primitive_name() else {
            return;
        };
        let mut held = self
            .connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let ids = held.entry(name.to_owned()).or_default();
        if let Some(evicted) = remember(ids, connection_id, MAX_CONNECTIONS_PER_PRIMITIVE) {
            debug!(
                primitive = name,
                connection = evicted,
                "Forgetting the oldest connection id: more than {MAX_CONNECTIONS_PER_PRIMITIVE} are held"
            );
        }
    }

    /// The connection ids currently held for a primitive.
    #[must_use]
    pub fn connections_for(&self, name: &str) -> Vec<usize> {
        self.connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
            .unwrap_or_default()
    }

    /// Close every connection admitted as this primitive.
    ///
    /// Returns how many were still open. Called off the policy's tasks, from
    /// the process manager's pid watcher.
    pub fn revoke_all(&self, name: &str) -> usize {
        let ids = self
            .connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(name)
            .unwrap_or_default();
        if ids.is_empty() {
            return 0;
        }
        let Some(revoker) = self.revoker.get() else {
            debug!(
                primitive = name,
                "Not revoking {} connection(s): no listener is installed",
                ids.len()
            );
            return 0;
        };
        let held = ids.len();
        let closed = ids
            .iter()
            .filter(|id| revoker.revoke(**id))
            .inspect(|id| debug!(primitive = name, connection = **id, "Revoked a connection"))
            .count();
        debug!(
            primitive = name,
            held,
            closed,
            "Revoking the connections held for {name}: {closed} of {held} were still open"
        );
        closed
    }
}

impl crate::ipc_policy::PolicyObserver for ConnectionRegistry {
    fn on_admitted(&self, identity: &ConnectionIdentity, connection_id: usize) {
        self.record(identity, connection_id);
    }
}

impl ChildExitObserver for ConnectionRegistry {
    fn on_child_exit(&self, name: &str, pid: u32) {
        let revoked = self.revoke_all(name);
        if revoked > 0 {
            info!(
                primitive = name,
                pid,
                connections = revoked,
                "Revoked {revoked} IPC connection(s) after {name} (pid {pid}) exited"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine's pid in every ancestry case below.
    const ENGINE: u32 = 1000;

    fn tree(pairs: &[(u32, u32)]) -> HashMap<u32, u32> {
        pairs.iter().copied().collect()
    }

    fn children(pairs: &[(u32, &str)]) -> HashMap<u32, String> {
        pairs
            .iter()
            .map(|(pid, name)| (*pid, (*name).to_string()))
            .collect()
    }

    fn primitive(name: &str) -> ConnectionIdentity {
        ConnectionIdentity::Primitive {
            name: name.to_string(),
        }
    }

    // ========================================================================
    // The identity type
    // ========================================================================

    #[test]
    fn a_managed_identity_reports_its_name_and_prints_as_that_name() {
        let managed = primitive("timer");
        assert_eq!(managed.primitive_name(), Some("timer"));
        assert!(managed.is_managed());
        assert_eq!(managed.to_string(), "timer");
    }

    #[test]
    fn an_unmanaged_identity_names_nobody_and_prints_its_pid() {
        let unmanaged = unmanaged_from(Some(4242), 1000);
        assert_eq!(unmanaged.primitive_name(), None);
        assert!(!unmanaged.is_managed());
        assert_eq!(unmanaged.to_string(), "<unmanaged pid 4242>");
        assert_eq!(unmanaged_from(None, 0).to_string(), "<unmanaged>");
    }

    // ========================================================================
    // The ancestry walk
    // ========================================================================

    #[test]
    fn a_directly_spawned_primitive_resolves_on_the_first_look() {
        // No parent lookup is needed at all, which is why a direct spawn still
        // resolves on a platform that cannot supply one.
        assert_eq!(
            resolve_ancestry(Some(2000), ENGINE, &children(&[(2000, "timer")]), &|_| {
                panic!("a direct match must not consult the process tree")
            }),
            Resolution::Primitive("timer".to_string())
        );
    }

    #[test]
    fn a_uv_launched_primitive_resolves_through_its_launcher() {
        // The measured shape: `uv run` stays alive as the engine's child and
        // the interpreter that opens the socket is its child.
        let parents = tree(&[(3_698_337, 3_698_333), (3_698_333, ENGINE)]);
        assert_eq!(
            resolve_ancestry(
                Some(3_698_337),
                ENGINE,
                &children(&[(3_698_333, "webhook_console")]),
                &|pid| parents.get(&pid).copied()
            ),
            Resolution::Primitive("webhook_console".to_string())
        );
    }

    #[test]
    fn a_wrapper_chain_of_any_depth_still_resolves() {
        let parents = tree(&[(5004, 5003), (5003, 5002), (5002, 5001), (5001, ENGINE)]);
        assert_eq!(
            resolve_ancestry(
                Some(5004),
                ENGINE,
                &children(&[(5001, "wrapped")]),
                &|pid| { parents.get(&pid).copied() }
            ),
            Resolution::Primitive("wrapped".to_string())
        );
    }

    #[test]
    fn a_process_the_engine_never_spawned_is_foreign() {
        let parents = tree(&[(9001, 9000), (9000, 1)]);
        assert_eq!(
            resolve_ancestry(Some(9001), ENGINE, &children(&[(2000, "timer")]), &|pid| {
                parents.get(&pid).copied()
            }),
            Resolution::Foreign
        );
    }

    #[test]
    fn the_walk_stops_at_the_engine_rather_than_claiming_it() {
        // A descendant of the engine the table does not name yet is worth one
        // more look. That is the startup race, and nothing else retries.
        let parents = tree(&[(2001, ENGINE)]);
        assert_eq!(
            resolve_ancestry(Some(2001), ENGINE, &HashMap::new(), &|pid| parents
                .get(&pid)
                .copied()),
            Resolution::UnrecordedChild
        );
        assert_eq!(
            resolve_ancestry(Some(ENGINE), ENGINE, &HashMap::new(), &|_| None),
            Resolution::UnrecordedChild,
            "the engine's own connection is never a primitive"
        );
    }

    #[test]
    fn a_child_the_engine_no_longer_holds_is_not_a_primitive() {
        // The pid-reuse argument as a decision: once the entry is gone the
        // same pid resolves to nobody, whatever it used to be. The table is in
        // that state from the moment `Child::wait` returns.
        let live = children(&[(2000, "timer")]);
        assert_eq!(
            resolve_ancestry(Some(2000), ENGINE, &live, &|_| None),
            Resolution::Primitive("timer".to_string())
        );
        assert_eq!(
            resolve_ancestry(Some(2000), ENGINE, &HashMap::new(), &|_| None),
            Resolution::Foreign,
            "a reaped child's pid names nobody, so whoever inherits it cannot borrow the name"
        );
    }

    #[test]
    fn a_missing_peer_pid_is_foreign() {
        assert_eq!(
            resolve_ancestry(None, ENGINE, &children(&[(2000, "timer")]), &|_| Some(
                ENGINE
            )),
            Resolution::Foreign
        );
    }

    #[test]
    fn a_parent_lookup_that_answers_nothing_collapses_to_a_direct_match() {
        // The platform without /proc: a direct spawn resolves, a launcher does
        // not. This is what `ancestry_available` warns about at startup.
        assert_eq!(
            resolve_ancestry(Some(2000), ENGINE, &children(&[(2000, "timer")]), &|_| None),
            Resolution::Primitive("timer".to_string())
        );
        assert_eq!(
            resolve_ancestry(Some(2001), ENGINE, &children(&[(2000, "timer")]), &|_| None),
            Resolution::Foreign
        );
    }

    #[test]
    fn a_cycle_in_the_process_tree_terminates() {
        let parents = tree(&[(7001, 7002), (7002, 7001)]);
        assert_eq!(
            resolve_ancestry(Some(7001), ENGINE, &HashMap::new(), &|pid| parents
                .get(&pid)
                .copied()),
            Resolution::Foreign
        );
    }

    #[test]
    fn pid_one_terminates_the_walk() {
        assert_eq!(
            resolve_ancestry(Some(1), ENGINE, &HashMap::new(), &|_| panic!(
                "pid 1 has no parent worth asking for"
            )),
            Resolution::Foreign
        );
    }

    // ========================================================================
    // Reading /proc
    // ========================================================================

    #[test]
    fn the_real_proc_reader_finds_this_processs_parent() {
        let Some(parent) = proc_parent_of(std::process::id()) else {
            // No /proc: the documented degradation, not a failure.
            assert!(!ancestry_available());
            return;
        };
        assert!(parent > 0, "a running process has a parent");
        assert!(ancestry_available());
    }

    #[test]
    fn a_process_name_containing_spaces_and_parens_does_not_confuse_the_parser() {
        // Field 2 is free-form, so the fields after it are counted from the
        // LAST ')'. This is the parsing `proc_parent_of` does, over a line
        // /proc could really produce for a process named "my app (v2) :)".
        let stat = "4242 (my app (v2) :)) S 777 4242 4242 0 -1 4194304 1 0";
        let ppid: Option<u32> = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(1))
            .and_then(|field| field.parse().ok());
        assert_eq!(ppid, Some(777));
    }

    // ========================================================================
    // Admission
    // ========================================================================

    #[test]
    fn a_spawned_primitive_is_admitted_in_every_mode() {
        for mode in [
            AuthenticationMode::Off,
            AuthenticationMode::Warn,
            AuthenticationMode::Strict,
        ] {
            assert_eq!(
                admission_for(mode, &primitive("timer"), Some(1000), 1000),
                AdmissionDecision::Admit,
                "mode {mode}"
            );
        }
    }

    #[test]
    fn what_each_mode_does_with_a_peer_the_engine_did_not_spawn() {
        let same_user = unmanaged_from(Some(4242), 1000);
        let other_user = unmanaged_from(Some(4242), 1001);
        let cases = [
            (
                "off lets anyone in",
                AuthenticationMode::Off,
                &same_user,
                Some(1000),
                true,
                false,
            ),
            (
                "off does not care about the uid",
                AuthenticationMode::Off,
                &other_user,
                Some(1001),
                true,
                false,
            ),
            (
                "warn notes a same-user peer",
                AuthenticationMode::Warn,
                &same_user,
                Some(1000),
                true,
                true,
            ),
            (
                "warn notes another user too",
                AuthenticationMode::Warn,
                &other_user,
                Some(1001),
                true,
                true,
            ),
            (
                "strict keeps the CLI working",
                AuthenticationMode::Strict,
                &same_user,
                Some(1000),
                true,
                true,
            ),
            (
                "strict refuses another user",
                AuthenticationMode::Strict,
                &other_user,
                Some(1001),
                false,
                false,
            ),
        ];
        for (case, mode, identity, peer_uid, admitted, noted) in cases {
            let decision = admission_for(mode, identity, peer_uid, 1000);
            assert_eq!(decision.is_admitted(), admitted, "{case}");
            assert_eq!(
                decision == AdmissionDecision::AdmitUnmanaged,
                noted,
                "{case}"
            );
        }
    }

    #[test]
    fn strict_refuses_a_peer_whose_credentials_the_platform_withheld() {
        let identity = unmanaged_from(None, 0);
        match admission_for(AuthenticationMode::Strict, &identity, None, 1000) {
            AdmissionDecision::Deny(reason) => {
                assert!(reason.contains("no peer credentials"), "{reason}");
            }
            other => panic!("strict cannot vouch for a peer it knows nothing about: {other:?}"),
        }
        assert_eq!(
            admission_for(AuthenticationMode::Off, &identity, None, 1000),
            AdmissionDecision::Admit,
            "the lenient modes still let it through"
        );
    }

    #[test]
    fn the_denial_names_both_user_ids() {
        match admission_for(
            AuthenticationMode::Strict,
            &unmanaged_from(Some(4242), 1001),
            Some(1001),
            1000,
        ) {
            AdmissionDecision::Deny(reason) => {
                assert!(reason.contains("uid 1000"), "{reason}");
                assert!(reason.contains("uid 1001"), "{reason}");
            }
            other => panic!("a different user id is refused under strict: {other:?}"),
        }
    }

    #[test]
    fn every_mode_spells_itself_the_way_the_config_key_does() {
        assert_eq!(AuthenticationMode::default(), AuthenticationMode::Off);
        assert!(!AuthenticationMode::Off.is_enforcing());
        for (mode, spelling) in [
            (AuthenticationMode::Off, "off"),
            (AuthenticationMode::Warn, "warn"),
            (AuthenticationMode::Strict, "strict"),
        ] {
            assert_eq!(mode.to_string(), spelling);
            assert_eq!(mode.as_str(), spelling);
        }
        assert!(AuthenticationMode::Warn.is_enforcing());
        assert!(AuthenticationMode::Strict.is_enforcing());
    }

    // ========================================================================
    // The resolver over a live child table
    // ========================================================================

    /// A child table the test controls, standing in for the process manager.
    struct Table(Mutex<HashMap<u32, String>>);

    impl Table {
        fn new(pairs: &[(u32, &str)]) -> Arc<Self> {
            Arc::new(Self(Mutex::new(children(pairs))))
        }

        fn insert(&self, pid: u32, name: &str) {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(pid, name.to_owned());
        }
    }

    #[async_trait::async_trait]
    impl SpawnedChildren for Table {
        async fn child_names_by_pid(&self) -> HashMap<u32, String> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    #[tokio::test]
    async fn the_resolver_names_a_child_the_table_already_holds() {
        let resolver =
            AncestryResolver::new(Table::new(&[(2000, "timer")]), AuthenticationMode::Off)
                .with_process_facts(ENGINE, 1000);
        assert_eq!(
            resolver.walk(Some(2000)).await,
            Resolution::Primitive("timer".to_string())
        );
    }

    #[tokio::test]
    async fn the_resolver_waits_for_a_child_the_table_does_not_name_yet() {
        // The startup race. The walk is driven with this process's own pid and
        // this process is told it is the engine, so the first look answers
        // UnrecordedChild and the table is filled in underneath.
        let table = Table::new(&[]);
        let pid = std::process::id();
        let resolver = AncestryResolver::new(table.clone(), AuthenticationMode::Off)
            .with_process_facts(pid, 1000)
            .with_admission_retry(40, Duration::from_millis(5));

        let filler = table.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            filler.insert(pid, "late");
        });

        assert_eq!(
            resolver.walk(Some(pid)).await,
            Resolution::Primitive("late".to_string()),
            "a pid recorded during the retry window is still named"
        );
    }

    #[tokio::test]
    async fn the_resolver_does_not_wait_on_a_peer_outside_the_engines_tree() {
        // Retrying a foreign peer would put the whole backoff in front of
        // every CLI query. pid 1 is outside any engine's tree.
        let resolver = AncestryResolver::new(Table::new(&[]), AuthenticationMode::Off)
            .with_process_facts(ENGINE, 1000)
            .with_admission_retry(5, Duration::from_secs(30));
        let started = std::time::Instant::now();
        assert_eq!(resolver.walk(Some(1)).await, Resolution::Foreign);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a foreign peer is answered on the first look"
        );
    }

    // ========================================================================
    // Revocation
    // ========================================================================

    /// Records what it was asked to close, and whether the id was still open.
    struct Closed {
        open: Mutex<Vec<usize>>,
        asked: Mutex<Vec<usize>>,
    }

    impl Closed {
        fn new(open: &[usize]) -> Arc<Self> {
            Arc::new(Self {
                open: Mutex::new(open.to_vec()),
                asked: Mutex::new(Vec::new()),
            })
        }

        fn asked(&self) -> Vec<usize> {
            self.asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl ConnectionRevoker for Closed {
        fn revoke(&self, connection_id: usize) -> bool {
            self.asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(connection_id);
            self.open
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(&connection_id)
        }
    }

    #[test]
    fn a_connection_set_keeps_the_newest_ids_and_never_repeats_one() {
        let mut ids = Vec::new();
        assert_eq!(remember(&mut ids, 7, 3), None);
        assert_eq!(remember(&mut ids, 7, 3), None, "a repeat is not a new id");
        assert_eq!(ids, vec![7]);
        assert_eq!(remember(&mut ids, 8, 3), None);
        assert_eq!(remember(&mut ids, 9, 3), None);
        assert_eq!(
            remember(&mut ids, 10, 3),
            Some(7),
            "the oldest id makes room"
        );
        assert_eq!(ids, vec![8, 9, 10]);
    }

    #[test]
    fn every_connection_a_primitive_opened_is_revoked_when_its_child_exits() {
        // Measured: one pid holds several connections, so the map is a name to
        // a set of ids and a child exit has to close all of them.
        let closed = Closed::new(&[1, 2, 3, 4]);
        let registry = ConnectionRegistry::new();
        registry.install_revoker(closed.clone());

        for id in [1, 2, 3] {
            registry.record(&primitive("timer"), id);
        }
        registry.record(&primitive("console"), 4);
        registry.record(&unmanaged_from(Some(999), 1000), 5);

        assert_eq!(registry.connections_for("timer"), vec![1, 2, 3]);
        registry.on_child_exit("timer", 2000);
        assert_eq!(closed.asked(), vec![1, 2, 3]);
        assert!(
            registry.connections_for("timer").is_empty(),
            "a revoked primitive holds nothing afterwards"
        );
        assert_eq!(
            registry.connections_for("console"),
            vec![4],
            "another primitive's connections are untouched"
        );
    }

    #[test]
    fn an_unmanaged_connection_is_never_recorded() {
        let registry = ConnectionRegistry::new();
        registry.record(&unmanaged_from(Some(4242), 1000), 9);
        assert_eq!(registry.revoke_all("<unmanaged pid 4242>"), 0);
        assert!(registry.connections_for("timer").is_empty());
    }

    #[test]
    fn revoking_before_the_listener_exists_drops_the_ids_rather_than_keeping_them() {
        // The registry is built before the listener, so this is reachable if a
        // child somehow dies in between. Keeping the ids would mean revoking
        // them later, by which time they could belong to somebody else.
        let registry = ConnectionRegistry::new();
        registry.record(&primitive("timer"), 1);
        assert_eq!(registry.revoke_all("timer"), 0);
        assert!(registry.connections_for("timer").is_empty());
    }

    #[test]
    fn revocation_counts_only_the_connections_that_were_still_open() {
        let closed = Closed::new(&[2]);
        let registry = ConnectionRegistry::new();
        registry.install_revoker(closed.clone());
        registry.record(&primitive("timer"), 1);
        registry.record(&primitive("timer"), 2);
        assert_eq!(registry.revoke_all("timer"), 1);
        assert_eq!(closed.asked(), vec![1, 2]);
    }
}
