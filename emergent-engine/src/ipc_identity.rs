//! Who the engine decides a connection is.
//!
//! # This file is a stub, and enforcement is weaker because of it
//!
//! [`ConnectionIdentity`] and [`IdentityResolver`] are the shape agreed with
//! issue #24, which owns this file. [`StubResolver`] is not that work: it
//! answers [`ConnectionIdentity::Unmanaged`] for every peer, because resolving
//! a peer to the primitive the engine spawned needs a process-ancestry walk,
//! pid-reuse handling and revocation on child exit, all of which are #24.
//!
//! What that costs today is written out in [`StubResolver`]'s own docs and in
//! `docs/configuration.md`: while every peer is `Unmanaged`, declaration
//! enforcement falls back to the `source` field the client writes itself, so it
//! catches a primitive's mistakes and not a client's lies. The policy in
//! [`crate::ipc_policy`] already refuses a trusted connection that names
//! another primitive; that path simply never fires until a real resolver
//! starts returning [`ConnectionIdentity::Primitive`].

use acton_reactive::ipc::{IpcAccessDenied, PeerCredentials};

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
        /// The peer's user id.
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

/// Resolves a connecting peer to an identity.
///
/// Implemented by #24 over the process manager's table of live children.
/// `admit` awaits this, so it may read shared engine state.
///
/// # Errors
///
/// Returns a denial to refuse the connection outright. A resolver that cannot
/// name a peer should answer [`ConnectionIdentity::Unmanaged`] rather than
/// deny, and leave the consequences to the enforcement mode.
#[async_trait::async_trait]
pub trait IdentityResolver: Send + Sync + std::panic::RefUnwindSafe + 'static {
    /// Decide who a connecting peer is.
    async fn resolve(
        &self,
        peer: Option<PeerCredentials>,
    ) -> Result<ConnectionIdentity, IpcAccessDenied>;
}

/// Answers [`ConnectionIdentity::Unmanaged`] for every peer.
///
/// This is the placeholder #23 ships so its branch compiles and merges first.
/// It is not a security control and does not pretend to be one.
///
/// # What is weaker while this is installed
///
/// Every connection is `Unmanaged`, so declaration enforcement has no
/// engine-established name to work from and falls back to the `source` field
/// the publishing client writes. That is enough to catch a typo, a drifted
/// declaration or a topic a primitive emits but never declared, which is what
/// enforcement is for. It is not enough to stop a client that claims another
/// primitive's name: anything that can open the socket can publish as `timer`
/// whatever `timer` declared. The socket remains the trust boundary until #24
/// replaces this file.
pub struct StubResolver;

#[async_trait::async_trait]
impl IdentityResolver for StubResolver {
    async fn resolve(
        &self,
        peer: Option<PeerCredentials>,
    ) -> Result<ConnectionIdentity, IpcAccessDenied> {
        Ok(unmanaged_from(
            peer.and_then(PeerCredentials::pid),
            peer.map_or(0, PeerCredentials::uid),
        ))
    }
}

/// Build the identity the stub gives every peer (pure function).
///
/// Split out from [`StubResolver::resolve`] because `PeerCredentials` has no
/// public constructor, so the mapping can only be tested through the values
/// read out of it.
#[must_use]
pub const fn unmanaged_from(pid: Option<u32>, uid: u32) -> ConnectionIdentity {
    ConnectionIdentity::Unmanaged { pid, uid }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_managed_identity_reports_its_name_and_prints_as_that_name() {
        let managed = ConnectionIdentity::Primitive {
            name: "timer".to_string(),
        };
        assert_eq!(managed.primitive_name(), Some("timer"));
        assert!(managed.is_managed());
        assert_eq!(managed.to_string(), "timer");
    }

    #[test]
    fn an_unmanaged_identity_names_nobody_and_prints_its_pid() {
        let unmanaged = ConnectionIdentity::Unmanaged {
            pid: Some(4242),
            uid: 1000,
        };
        assert_eq!(unmanaged.primitive_name(), None);
        assert!(!unmanaged.is_managed());
        assert_eq!(unmanaged.to_string(), "<unmanaged pid 4242>");

        let no_pid = ConnectionIdentity::Unmanaged { pid: None, uid: 0 };
        assert_eq!(no_pid.to_string(), "<unmanaged>");
    }

    #[tokio::test]
    async fn the_stub_resolver_manages_nobody() {
        // Stated as a test so that replacing this file with #24's version
        // fails loudly here rather than quietly changing enforcement.
        let resolved = StubResolver.resolve(None).await;
        assert_eq!(
            resolved,
            Ok(ConnectionIdentity::Unmanaged { pid: None, uid: 0 })
        );
    }

    #[test]
    fn the_stub_carries_the_peer_it_was_given_into_the_identity() {
        // `PeerCredentials` can only be built by acton from a live socket, so
        // the values it reports are fed to the mapping directly here and the
        // reading of them is exercised by the live proof.
        assert_eq!(
            unmanaged_from(Some(4242), 1000),
            ConnectionIdentity::Unmanaged {
                pid: Some(4242),
                uid: 1000
            }
        );
        assert_eq!(
            unmanaged_from(None, 0),
            ConnectionIdentity::Unmanaged { pid: None, uid: 0 }
        );
        assert!(
            !unmanaged_from(Some(4242), 1000).is_managed(),
            "the stub names nobody, whatever the kernel reported"
        );
    }
}
