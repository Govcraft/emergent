//! Bind each IPC connection to the primitive that opened it, and hold every
//! operation on that connection to what the primitive declared.
//!
//! acton-reactive 9.4.0 admits a connection through an application policy and
//! then hands that policy every operation the connection attempts, together
//! with the identity admission established. That identity comes from the
//! kernel, not from the client, which is what makes enforcement mean something:
//! before 9.4.0 the only name available was the `source` field the publisher
//! wrote itself, so the engine could catch a mistake but never a lie.
//!
//! # Identifying a primitive from a connection
//!
//! The kernel reports the peer's pid. The engine knows the pid of every child
//! it spawned. Those two are often the same process, but not always: a
//! primitive configured as `path = "uv"` has `uv` as the engine's child, and
//! `uv` does not replace itself with the interpreter, so the process that
//! actually opens the socket is a grandchild the engine has never heard of.
//! Admission therefore walks up the process ancestry from the peer pid until it
//! reaches a pid the engine spawned, which covers a wrapper of any depth, and
//! gives up at the engine itself.
//!
//! [`resolve_identity`] is pure and takes the parent lookup as an argument.
//! [`proc_parent_of`] is the real one, and reads `/proc`. See its docs for what
//! happens where `/proc` does not exist.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::ipc::{
    IpcAccessDenied, IpcAdmission, IpcConnectionContext, IpcConnectionInfo, IpcEnvelope,
    IpcIdentity, IpcOperation, IpcSecurityPolicy,
};
use tracing::{debug, warn};

use crate::declarations::{
    Checked, DeclarationTable, Enforcement, EnforcementMode, Operation, RejectionReport,
};

/// The trusted identity of a connection, established once at admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrimitiveIdentity {
    /// The connection was traced to a primitive the engine spawned.
    Named(String),
    /// The peer could not be tied to any configured primitive.
    ///
    /// A client the engine did not spawn, a primitive whose pid the engine has
    /// not recorded yet, or a platform where the ancestry walk is unavailable.
    Unauthenticated,
}

impl PrimitiveIdentity {
    /// How an unidentified connection is named in a log line or a report.
    ///
    /// Deliberately not a name any primitive could have: `<` and `>` are not
    /// legal in a primitive name, so nothing a client controls can collide
    /// with it and nothing derived from it forms a valid message type.
    pub const UNAUTHENTICATED: &'static str = "<unauthenticated>";

    /// The primitive's name, when one was established.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Named(name) => Some(name),
            Self::Unauthenticated => None,
        }
    }

    /// Whether admission tied this connection to a configured primitive.
    #[must_use]
    pub const fn is_named(&self) -> bool {
        matches!(self, Self::Named(_))
    }
}

impl std::fmt::Display for PrimitiveIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Named(name) => f.write_str(name),
            Self::Unauthenticated => f.write_str(Self::UNAUTHENTICATED),
        }
    }
}

/// How far the ancestry walk climbs before giving up.
///
/// A wrapper chain is one or two processes deep in practice (`uv` to `python`,
/// or a shell to a binary). The bound is what keeps a `/proc` that reports a
/// cycle, or a pid recycled mid-walk, from spinning.
pub const MAX_ANCESTRY_DEPTH: usize = 16;

/// What the ancestry walk found, which is more than just the identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The connection belongs to this primitive.
    Primitive(String),
    /// The walk reached the engine without passing a pid the engine has
    /// recorded, so the peer is a child of the engine that the process table
    /// does not name yet. Another look shortly is worth taking.
    UnrecordedChild,
    /// The walk left the engine's process tree, or could not start. Nothing
    /// about this peer will change by looking again.
    Foreign,
}

impl Resolution {
    /// The identity this resolution establishes.
    #[must_use]
    pub fn identity(self) -> PrimitiveIdentity {
        match self {
            Self::Primitive(name) => PrimitiveIdentity::Named(name),
            Self::UnrecordedChild | Self::Foreign => PrimitiveIdentity::Unauthenticated,
        }
    }
}

/// Tie a connecting process to the primitive the engine spawned for it.
///
/// Walks from `peer_pid` up through parents, returning the first pid found in
/// `spawned`. Stops at the engine itself, at pid 1, at the first parent the
/// lookup cannot supply, and after [`MAX_ANCESTRY_DEPTH`] steps.
///
/// Pure: `parent_of` is the only way it learns about the process tree.
#[must_use]
pub fn resolve_identity(
    peer_pid: Option<u32>,
    engine_pid: u32,
    spawned: &HashMap<u32, String>,
    parent_of: &dyn Fn(u32) -> Option<u32>,
) -> Resolution {
    let Some(mut pid) = peer_pid else {
        return Resolution::Foreign;
    };

    for _ in 0..MAX_ANCESTRY_DEPTH {
        if let Some(name) = spawned.get(&pid) {
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

/// Reads a process's parent from `/proc`, where `/proc` exists.
///
/// On Linux this is `/proc/<pid>/stat` field 4. Everywhere else it reports no
/// parent, which collapses the walk to a direct pid match: a primitive the
/// engine exec'd itself still resolves, and one behind a wrapper such as `uv`
/// resolves as [`Resolution::Foreign`].
#[must_use]
pub fn proc_parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The second field is the executable name in parentheses and may itself
    // contain spaces or parentheses, so fields are counted after the last ')'.
    let after_comm = stat.rsplit_once(')')?.1;
    // After the name come: state, ppid, ...
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

/// A snapshot of the pids the engine has spawned, keyed by primitive name.
///
/// Admission is the only asynchronous hook acton offers, so this is where the
/// engine's live process table can be consulted. `authorize` is synchronous and
/// must not block, which is why the identity is resolved once here and then
/// carried on the connection for its lifetime.
pub trait SpawnedPids: Send + Sync + 'static {
    /// The pid of every primitive the engine currently believes it is running.
    fn snapshot(&self) -> Pin<Box<dyn Future<Output = HashMap<u32, String>> + Send + '_>>;
}

/// Observes what the policy decided, for side effects the policy itself avoids.
///
/// Every method is called from acton's connection or broker task. `authorize`
/// must be fast and nonblocking, so an observer must hand work off rather than
/// do it inline: send on a channel, bump a counter, set a flag. It must not
/// block, await, or panic.
pub trait PolicyObserver: Send + Sync + 'static {
    /// A connection was admitted and bound to this identity.
    fn on_admitted(&self, _identity: &PrimitiveIdentity, _connection_id: usize) {}

    /// A primitive was allowed to take on these subscription topics.
    fn on_subscribed(&self, _identity: &PrimitiveIdentity, _topics: &[String]) {}

    /// An operation fell outside the declarations, in warn or strict mode.
    ///
    /// `enforcement` says what the engine did about it:
    /// [`Enforcement::Warn`] means the operation still went through.
    fn on_violation(&self, _report: &RejectionReport, _enforcement: Enforcement) {}
}

/// Holds every connection to what its primitive declared.
///
/// Installed only when it has something to do: see [`policy_is_needed`]. When
/// it is not installed the engine starts its listener exactly as it always did,
/// so the default configuration pays nothing, including on acton's per
/// notification [`IpcOperation::Deliver`] path.
pub struct EnginePolicy {
    declarations: Arc<DeclarationTable>,
    pids: Arc<dyn SpawnedPids>,
    observer: Arc<dyn PolicyObserver>,
    engine_pid: u32,
    /// How many times admission re-reads the process table before giving up.
    ///
    /// A primitive connects moments after the engine spawns it, and the engine
    /// records the pid on its own task, so a fast primitive can arrive before
    /// the table names it. Re-reading costs nothing when the first look wins.
    admission_attempts: u32,
    admission_backoff: Duration,
}

/// Whether the engine needs to install a policy at all (pure function).
///
/// Enforcement needs one. So does an observer, which is how another part of the
/// engine can watch admissions and subscriptions while enforcement stays off.
#[must_use]
pub const fn policy_is_needed(mode: EnforcementMode, has_observer: bool) -> bool {
    mode.is_enforcing() || has_observer
}

// A policy's own state is fixed once it is built: the declaration table, the
// process-table reader, the observer and the retry settings are never mutated,
// and `authorize` takes `&self`. Nothing the policy owns can be left half
// written by a panic, so observing it after one is sound. The engine state
// reached through `pids` is read, never modified, and its own consistency is
// the process manager's business, not the policy's.
impl std::panic::RefUnwindSafe for EnginePolicy {}

/// An observer that does nothing, used when nobody registered one.
struct NoObserver;
impl PolicyObserver for NoObserver {}

impl EnginePolicy {
    /// Build a policy over the declarations and the engine's process table.
    #[must_use]
    pub fn new(
        declarations: Arc<DeclarationTable>,
        pids: Arc<dyn SpawnedPids>,
        observer: Option<Arc<dyn PolicyObserver>>,
    ) -> Self {
        Self {
            declarations,
            pids,
            observer: observer.unwrap_or_else(|| Arc::new(NoObserver)),
            engine_pid: std::process::id(),
            admission_attempts: DEFAULT_ADMISSION_ATTEMPTS,
            admission_backoff: DEFAULT_ADMISSION_BACKOFF,
        }
    }

    /// Override how patiently admission waits for a pid to appear.
    #[must_use]
    pub const fn with_admission_retry(mut self, attempts: u32, backoff: Duration) -> Self {
        self.admission_attempts = attempts;
        self.admission_backoff = backoff;
        self
    }

    /// Resolve a connecting pid to a primitive, waiting briefly for a late pid.
    ///
    /// A primitive connects moments after the engine spawns it, and the engine
    /// records the pid on its own task, so the first look can miss a fast
    /// primitive. Each retry costs a read of the process table, and only a
    /// connection that has not been identified yet ever retries.
    async fn identify(&self, peer_pid: Option<u32>) -> (PrimitiveIdentity, u32) {
        for attempt in 0..self.admission_attempts {
            let spawned = self.pids.snapshot().await;
            let resolution = resolve_identity(peer_pid, self.engine_pid, &spawned, &|pid| {
                proc_parent_of(pid)
            });
            // Only a child of the engine that the table does not name yet is
            // worth waiting for. A client from outside the engine's process
            // tree will never appear there, so it is admitted at once and a
            // topology query from a CLI pays nothing for enforcement.
            if resolution != Resolution::UnrecordedChild || attempt + 1 == self.admission_attempts {
                return (resolution.identity(), attempt + 1);
            }
            tokio::time::sleep(self.admission_backoff).await;
        }
        (PrimitiveIdentity::Unauthenticated, self.admission_attempts)
    }

    /// The Emergent message type a publish request carries, if it carries one.
    ///
    /// The envelope's own `message_type` is the registered wire name, always
    /// `"EmergentMessage"` here; the Emergent type is one level inside the
    /// payload. `payload` is already a `serde_json::Value`, so this is two
    /// borrows and no parsing.
    fn published_type(envelope: &IpcEnvelope) -> Option<&str> {
        envelope.payload.get("inner")?.get("message_type")?.as_str()
    }

    /// The `source` a publish request claims, if it claims one.
    fn claimed_source(envelope: &IpcEnvelope) -> Option<&str> {
        let source = envelope.payload.get("inner")?.get("source")?.as_str()?;
        (!source.is_empty()).then_some(source)
    }

    /// Turn a violation into a denial, after telling the observer about it.
    fn deny(
        &self,
        identity: &PrimitiveIdentity,
        checked: Checked,
        operation: Operation,
        message_type: &str,
    ) -> Result<(), IpcAccessDenied> {
        let report = RejectionReport::new(
            checked.verdict,
            &identity.to_string(),
            operation,
            message_type,
            self.declarations.mode(),
        );
        warn!(
            primitive = %identity,
            operation = operation.as_str(),
            message.type = %message_type,
            mode = %self.declarations.mode(),
            "Declaration violation: {}",
            report.reason
        );
        self.observer.on_violation(&report, checked.enforcement);
        match checked.enforcement {
            Enforcement::Accept | Enforcement::Warn => Ok(()),
            Enforcement::Reject => Err(IpcAccessDenied::new(report.reason)),
        }
    }

    /// Check one publish request.
    fn authorize_request(
        &self,
        identity: &PrimitiveIdentity,
        envelope: &IpcEnvelope,
    ) -> Result<(), IpcAccessDenied> {
        // Anything that is not an Emergent publish is not a declaration
        // question: acton's own traffic and the engine's internal requests pass
        // straight through.
        let Some(message_type) = Self::published_type(envelope) else {
            return Ok(());
        };
        let name = identity.name();

        let claimed = Self::claimed_source(envelope);
        let source_check = self.declarations.check_source(name, claimed);
        if source_check.verdict.is_violation() {
            return self.deny(identity, source_check, Operation::Publish, message_type);
        }

        let checked = self
            .declarations
            .check(name, Operation::Publish, message_type);
        if checked.verdict.is_violation() {
            return self.deny(identity, checked, Operation::Publish, message_type);
        }
        Ok(())
    }

    /// Check one batch of subscription topics.
    ///
    /// acton applies a batch atomically, so the batch is judged as a whole: the
    /// first topic outside the declarations denies all of them, and nothing is
    /// applied. The denial names that topic so the operator knows which.
    fn authorize_subscribe(
        &self,
        identity: &PrimitiveIdentity,
        topics: &[String],
    ) -> Result<(), IpcAccessDenied> {
        let name = identity.name();
        for topic in topics {
            let checked = self.declarations.check(name, Operation::Subscribe, topic);
            if checked.verdict.is_violation() {
                self.deny(identity, checked, Operation::Subscribe, topic)?;
            }
        }
        self.observer.on_subscribed(identity, topics);
        Ok(())
    }
}

/// Admission re-reads the process table this many times before giving up.
const DEFAULT_ADMISSION_ATTEMPTS: u32 = 5;
/// How long admission waits between those reads.
const DEFAULT_ADMISSION_BACKOFF: Duration = Duration::from_millis(40);

impl IpcSecurityPolicy for EnginePolicy {
    fn admit(&self, connection: IpcConnectionInfo) -> IpcAdmission<'_> {
        Box::pin(async move {
            let peer = connection.peer_credentials().and_then(|p| p.pid());
            let (identity, looks) = self.identify(peer).await;

            if identity.is_named() {
                debug!(
                    primitive = %identity,
                    peer.pid = ?peer,
                    connection = connection.connection_id(),
                    looks,
                    "Admitted an IPC connection"
                );
            } else {
                // Never refused here: whether an unidentified client may do
                // anything is the enforcement mode's decision, not admission's,
                // so `off` keeps behaving exactly as it always has.
                debug!(
                    peer.pid = ?peer,
                    connection = connection.connection_id(),
                    looks,
                    "Admitted an IPC connection the engine could not tie to a primitive"
                );
            }

            self.observer
                .on_admitted(&identity, connection.connection_id());
            Ok(IpcIdentity::new(identity))
        })
    }

    fn authorize(
        &self,
        context: &IpcConnectionContext,
        operation: IpcOperation<'_>,
    ) -> Result<(), IpcAccessDenied> {
        let identity = context
            .identity::<PrimitiveIdentity>()
            .unwrap_or(&PrimitiveIdentity::Unauthenticated);

        match operation {
            IpcOperation::Request(envelope) => self.authorize_request(identity, envelope),
            IpcOperation::Subscribe(topics) | IpcOperation::SubscribePatterns(topics) => {
                self.authorize_subscribe(identity, topics)
            }
            // Dropping a subscription, asking what exists, and receiving a
            // notification are not declaration questions. Delivery in
            // particular is evaluated per notification per subscriber on the
            // broker's own task, and is redundant once subscribing is bound, so
            // it stays a single match arm.
            IpcOperation::Unsubscribe(_)
            | IpcOperation::UnsubscribePatterns(_)
            | IpcOperation::Discover
            | IpcOperation::Deliver(_) => Ok(()),
            // acton marks IpcOperation non-exhaustive, so a version that adds
            // an operation must not silently start denying it.
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A process tree as child -> parent, for the pure walk.
    fn tree(pairs: &[(u32, u32)]) -> HashMap<u32, u32> {
        pairs.iter().copied().collect()
    }

    fn spawned(pairs: &[(u32, &str)]) -> HashMap<u32, String> {
        pairs
            .iter()
            .map(|(pid, name)| (*pid, (*name).to_string()))
            .collect()
    }

    #[test]
    fn a_directly_spawned_primitive_resolves_immediately() {
        let children = spawned(&[(100, "timer")]);
        let parents = tree(&[(100, 10)]);
        assert_eq!(
            resolve_identity(Some(100), 10, &children, &|pid| parents.get(&pid).copied()),
            Resolution::Primitive("timer".to_string())
        );
    }

    #[test]
    fn a_primitive_behind_a_wrapper_resolves_through_its_ancestry() {
        // What `path = "uv"` actually produces: engine 10 spawned uv 200, and
        // uv spawned the python 201 that opens the socket.
        let children = spawned(&[(200, "webhook")]);
        let parents = tree(&[(201, 200), (200, 10)]);
        assert_eq!(
            resolve_identity(Some(201), 10, &children, &|pid| parents.get(&pid).copied()),
            Resolution::Primitive("webhook".to_string())
        );
    }

    #[test]
    fn a_deeper_wrapper_chain_still_resolves() {
        let children = spawned(&[(200, "webhook")]);
        let parents = tree(&[(203, 202), (202, 201), (201, 200), (200, 10)]);
        assert_eq!(
            resolve_identity(Some(203), 10, &children, &|pid| parents.get(&pid).copied()),
            Resolution::Primitive("webhook".to_string())
        );
    }

    #[test]
    fn a_process_the_engine_never_spawned_is_unauthenticated() {
        let children = spawned(&[(100, "timer")]);
        // 500 is someone else's process, parented outside the engine.
        let parents = tree(&[(500, 400), (400, 1)]);
        assert_eq!(
            resolve_identity(Some(500), 10, &children, &|pid| parents.get(&pid).copied()),
            Resolution::Foreign
        );
    }

    #[test]
    fn the_walk_stops_at_the_engine_rather_than_claiming_it() {
        let children = spawned(&[(100, "timer")]);
        let parents = tree(&[(300, 10), (10, 1)]);
        // A process the engine spawned but has not recorded yet, such as one
        // still starting, borrows no name. It is reported as the one case
        // worth looking at again in a moment.
        let pending = resolve_identity(Some(300), 10, &children, &|pid| parents.get(&pid).copied());
        assert_eq!(pending, Resolution::UnrecordedChild);
        assert_eq!(pending.identity(), PrimitiveIdentity::Unauthenticated);
        // And the engine's own connection resolves to nothing.
        assert_eq!(
            resolve_identity(Some(10), 10, &children, &|pid| parents.get(&pid).copied()),
            Resolution::UnrecordedChild
        );
    }

    #[test]
    fn a_missing_peer_pid_is_unauthenticated() {
        let children = spawned(&[(100, "timer")]);
        assert_eq!(
            resolve_identity(None, 10, &children, &|_| None),
            Resolution::Foreign
        );
    }

    #[test]
    fn a_parent_lookup_that_answers_nothing_collapses_to_a_direct_match() {
        // The non-Linux case: no ancestry, so only an exec'd child resolves.
        let children = spawned(&[(100, "timer"), (200, "webhook")]);
        assert_eq!(
            resolve_identity(Some(100), 10, &children, &|_| None),
            Resolution::Primitive("timer".to_string())
        );
        assert_eq!(
            resolve_identity(Some(201), 10, &children, &|_| None),
            Resolution::Foreign
        );
    }

    #[test]
    fn a_cycle_in_the_process_tree_terminates() {
        let children = spawned(&[(100, "timer")]);
        let parents = tree(&[(500, 501), (501, 500)]);
        assert_eq!(
            resolve_identity(Some(500), 10, &children, &|pid| parents.get(&pid).copied()),
            Resolution::Foreign
        );
    }

    #[test]
    fn pid_one_terminates_the_walk() {
        let children = spawned(&[(100, "timer")]);
        let parents = tree(&[(2, 1)]);
        assert_eq!(
            resolve_identity(Some(2), 10, &children, &|pid| parents.get(&pid).copied()),
            Resolution::Foreign
        );
    }

    #[test]
    fn the_real_proc_reader_finds_this_processs_parent() {
        let me = std::process::id();
        let parent = proc_parent_of(me);
        if cfg!(target_os = "linux") {
            assert!(parent.is_some(), "/proc should report a parent for {me}");
            assert_ne!(parent, Some(me));
        }
        // A pid that cannot exist has no parent on any platform.
        assert_eq!(proc_parent_of(u32::MAX), None);
    }

    #[test]
    fn a_process_name_containing_spaces_or_parens_does_not_confuse_the_parser() {
        // /proc/<pid>/stat field 2 is "(comm)" and comm can contain both, which
        // is why the parser splits on the last ')' rather than on whitespace.
        let stat = "1234 (weird )name( here) S 99 1234 1234 0 -1 4194304";
        let after_comm = stat.rsplit_once(')').map(|(_, rest)| rest).unwrap_or("");
        let ppid: Option<u32> = after_comm
            .split_whitespace()
            .nth(1)
            .and_then(|f| f.parse().ok());
        assert_eq!(ppid, Some(99));
    }

    #[test]
    fn identity_reports_its_name_and_prints_readably() {
        let named = Resolution::Primitive("timer".to_string()).identity();
        assert_eq!(named.name(), Some("timer"));
        assert!(named.is_named());
        assert_eq!(named.to_string(), "timer");

        let anon = PrimitiveIdentity::Unauthenticated;
        assert_eq!(anon.name(), None);
        assert!(!anon.is_named());
        assert_eq!(anon.to_string(), "<unauthenticated>");
        assert_eq!(anon.to_string(), PrimitiveIdentity::UNAUTHENTICATED);
        assert_eq!(
            Resolution::UnrecordedChild.identity(),
            PrimitiveIdentity::Unauthenticated
        );
        assert_eq!(
            Resolution::Foreign.identity(),
            PrimitiveIdentity::Unauthenticated
        );
    }

    // ========================================================================
    // The policy over the declarations
    // ========================================================================

    use crate::declarations::Declarations;
    use crate::primitives::PrimitiveKind;
    use std::sync::Mutex;

    /// A process table that reports what the test tells it to, one look at a time.
    struct ScriptedPids(Mutex<Vec<HashMap<u32, String>>>);

    impl ScriptedPids {
        fn steady(pairs: &[(u32, &str)]) -> Self {
            Self(Mutex::new(vec![table(pairs)]))
        }
    }

    fn table(pairs: &[(u32, &str)]) -> HashMap<u32, String> {
        pairs
            .iter()
            .map(|(pid, name)| (*pid, (*name).to_owned()))
            .collect()
    }

    impl SpawnedPids for ScriptedPids {
        fn snapshot(&self) -> Pin<Box<dyn Future<Output = HashMap<u32, String>> + Send + '_>> {
            Box::pin(async move {
                let mut looks = self.0.lock().unwrap_or_else(|e| e.into_inner());
                if looks.len() > 1 {
                    looks.remove(0)
                } else {
                    looks.first().cloned().unwrap_or_default()
                }
            })
        }
    }

    /// Records every violation the policy reported.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, Enforcement)>>);

    impl Recorder {
        fn seen(&self) -> Vec<(String, Enforcement)> {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    impl PolicyObserver for Recorder {
        fn on_violation(&self, report: &RejectionReport, enforcement: Enforcement) {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((report.message_type.clone(), enforcement));
        }
    }

    /// The text of a denial, failing the test when the operation was allowed.
    fn denial(result: Result<(), IpcAccessDenied>, expectation: &str) -> String {
        match result {
            Ok(()) => panic!("{expectation}"),
            Err(denied) => denied.to_string(),
        }
    }

    fn declarations(mode: EnforcementMode) -> Arc<DeclarationTable> {
        Arc::new(DeclarationTable::new(
            mode,
            [
                (
                    "timer".to_owned(),
                    Declarations::new(PrimitiveKind::Source, &["timer.tick".to_owned()], &[]),
                ),
                (
                    "console".to_owned(),
                    Declarations::new(PrimitiveKind::Sink, &[], &["timer.*".to_owned()]),
                ),
            ],
        ))
    }

    fn policy(mode: EnforcementMode) -> (EnginePolicy, Arc<Recorder>) {
        let recorder = Arc::new(Recorder::default());
        let policy = EnginePolicy::new(
            declarations(mode),
            Arc::new(ScriptedPids::steady(&[])),
            Some(recorder.clone()),
        );
        (policy, recorder)
    }

    fn publish(source: &str, message_type: &str) -> IpcEnvelope {
        IpcEnvelope::new(
            "message_broker",
            "EmergentMessage",
            serde_json::json!({
                "inner": { "source": source, "message_type": message_type }
            }),
        )
    }

    #[test]
    fn a_policy_is_installed_only_when_it_has_something_to_do() {
        let cases = [
            (EnforcementMode::Off, false, false),
            (EnforcementMode::Off, true, true),
            (EnforcementMode::Warn, false, true),
            (EnforcementMode::Strict, false, true),
        ];
        for (mode, has_observer, expected) in cases {
            assert_eq!(
                policy_is_needed(mode, has_observer),
                expected,
                "mode {mode} with observer {has_observer}"
            );
        }
    }

    #[test]
    fn a_declared_publish_is_authorized_in_every_mode() {
        for mode in [
            EnforcementMode::Off,
            EnforcementMode::Warn,
            EnforcementMode::Strict,
        ] {
            let (policy, recorder) = policy(mode);
            let identity = PrimitiveIdentity::Named("timer".to_owned());
            assert!(
                policy
                    .authorize_request(&identity, &publish("timer", "timer.tick"))
                    .is_ok(),
                "mode {mode}"
            );
            assert!(recorder.seen().is_empty(), "mode {mode}");
        }
    }

    #[test]
    fn an_undeclared_publish_is_reported_in_warn_and_refused_in_strict() {
        let cases = [
            (EnforcementMode::Off, true, 0),
            (EnforcementMode::Warn, true, 1),
            (EnforcementMode::Strict, false, 1),
        ];
        for (mode, allowed, reports) in cases {
            let (policy, recorder) = policy(mode);
            let identity = PrimitiveIdentity::Named("timer".to_owned());
            let decision =
                policy.authorize_request(&identity, &publish("timer", "timer.undeclared"));
            assert_eq!(decision.is_ok(), allowed, "mode {mode}");
            assert_eq!(recorder.seen().len(), reports, "mode {mode}");
        }
    }

    #[test]
    fn a_refusal_carries_the_reason_the_engine_logged() {
        let (policy, _) = policy(EnforcementMode::Strict);
        let identity = PrimitiveIdentity::Named("timer".to_owned());
        let denied = denial(
            policy.authorize_request(&identity, &publish("timer", "timer.undeclared")),
            "an undeclared publish is refused in strict mode",
        );
        assert_eq!(
            denied,
            "'timer' tried to publish 'timer.undeclared', which is not in its declared publishes list"
        );
    }

    #[test]
    fn a_publish_may_not_name_a_source_other_than_its_own_connection() {
        let (policy, _) = policy(EnforcementMode::Strict);
        let identity = PrimitiveIdentity::Named("timer".to_owned());
        let denied = denial(
            policy.authorize_request(&identity, &publish("console", "timer.tick")),
            "a spoofed source is refused in strict mode",
        );
        assert!(denied.contains("named a different source"), "{denied}");
    }

    #[test]
    fn a_request_that_is_not_an_emergent_publish_is_not_a_declaration_question() {
        let (policy, recorder) = policy(EnforcementMode::Strict);
        let identity = PrimitiveIdentity::Named("timer".to_owned());
        let envelope = IpcEnvelope::new(
            "message_broker",
            "SystemEvent",
            serde_json::json!({ "event": "started" }),
        );
        assert!(policy.authorize_request(&identity, &envelope).is_ok());
        assert!(recorder.seen().is_empty());
    }

    #[test]
    fn an_unidentified_connection_may_still_ask_the_engine_a_protocol_question() {
        let (policy, recorder) = policy(EnforcementMode::Strict);
        let anon = PrimitiveIdentity::Unauthenticated;
        assert!(
            policy
                .authorize_request(&anon, &publish("", "system.request.topology"))
                .is_ok()
        );
        assert!(
            policy
                .authorize_request(&anon, &publish("", "timer.tick"))
                .is_err(),
            "an unidentified connection may not publish ordinary traffic in strict mode"
        );
        assert_eq!(recorder.seen().len(), 1);
    }

    #[test]
    fn a_subscribe_batch_is_refused_whole_when_one_topic_is_undeclared() {
        let (policy, recorder) = policy(EnforcementMode::Strict);
        let identity = PrimitiveIdentity::Named("console".to_owned());
        assert!(
            policy
                .authorize_subscribe(&identity, &["timer.tick".to_owned()])
                .is_ok()
        );
        let denied = denial(
            policy.authorize_subscribe(
                &identity,
                &["timer.tick".to_owned(), "secrets.all".to_owned()],
            ),
            "a batch containing an undeclared topic is refused",
        );
        assert!(denied.contains("'secrets.all'"), "{denied}");
        assert_eq!(
            recorder.seen(),
            vec![("secrets.all".to_owned(), Enforcement::Reject)]
        );
    }

    #[test]
    fn a_source_may_not_subscribe_at_all() {
        let (policy, _) = policy(EnforcementMode::Strict);
        let identity = PrimitiveIdentity::Named("timer".to_owned());
        let denied = denial(
            policy.authorize_subscribe(&identity, &["timer.tick".to_owned()]),
            "a source has nothing it may subscribe to",
        );
        assert!(denied.contains("its kind cannot subscribe"), "{denied}");
    }

    #[tokio::test]
    async fn admission_waits_for_a_child_the_engine_has_not_recorded_yet() {
        // The test process stands in for a child whose pid the engine records
        // late: the walk reaches the engine at once, which is the one case
        // worth another look.
        let late = std::process::id();
        let pids = Arc::new(ScriptedPids(Mutex::new(vec![
            HashMap::new(),
            HashMap::new(),
            table(&[(late, "timer")]),
        ])));
        let policy = EnginePolicy::new(declarations(EnforcementMode::Strict), pids, None)
            .with_admission_retry(5, Duration::from_millis(1));
        assert_eq!(
            policy.identify(Some(late)).await,
            (PrimitiveIdentity::Named("timer".to_owned()), 3),
            "the third look is the one that finds it"
        );
    }

    #[tokio::test]
    async fn admission_spends_every_attempt_on_a_child_that_never_appears() {
        let policy = EnginePolicy::new(
            declarations(EnforcementMode::Strict),
            Arc::new(ScriptedPids::steady(&[])),
            None,
        )
        .with_admission_retry(2, Duration::from_millis(1));
        assert_eq!(
            policy.identify(Some(std::process::id())).await,
            (PrimitiveIdentity::Unauthenticated, 2),
        );
    }

    #[tokio::test]
    async fn admission_does_not_wait_for_a_client_from_outside_the_engines_tree() {
        // A CLI or a topology viewer is never going to appear in the process
        // table, so enforcement must not add latency to its connection.
        let policy = EnginePolicy::new(
            declarations(EnforcementMode::Strict),
            Arc::new(ScriptedPids::steady(&[])),
            None,
        )
        .with_admission_retry(5, Duration::from_secs(30));
        assert_eq!(
            policy.identify(Some(1)).await,
            (PrimitiveIdentity::Unauthenticated, 1),
            "one look settles a peer outside the engine's process tree"
        );
    }
}
