//! Hold every IPC connection to what its primitive declared.
//!
//! acton-reactive 9.4.0 admits a connection through an application policy and
//! then hands that policy every operation the connection attempts, together
//! with the identity admission established. This module is that policy.
//!
//! # The two halves, and who owns them
//!
//! `admit` asks an [`IdentityResolver`] who the peer is and binds the answer to
//! the connection for its lifetime. Resolving a peer to a spawned primitive is
//! issue #24 and lives in [`crate::ipc_identity`]; this file only asks.
//!
//! `authorize` is issue #23: it holds each operation to the `publishes` and
//! `subscribes` lists in `emergent.toml`. It is synchronous and is called on
//! acton's connection task, so every decision here is a lookup against a table
//! built once at startup, and anything with a side effect is handed off.
//!
//! # Trusted and untrusted names
//!
//! Enforcement needs a name. A [`ConnectionIdentity::Primitive`] name came from
//! the engine and the kernel, so a message on that connection claiming a
//! different `source` is a violation in its own right. A
//! [`ConnectionIdentity::Unmanaged`] connection has no such name, so the only
//! name available is the `source` the client wrote itself, and enforcement
//! falls back to it: mistakes are caught, lies are not. While
//! [`crate::ipc_identity::StubResolver`] is installed that is every connection.
//! See [`effective_name`], where the choice is made and tested.
//!
//! A subscribe frame carries no `source`, so there is nothing to fall back to
//! and subscribe enforcement is dormant until a resolver names connections.
//! The engine says so at startup rather than refusing every handler and sink.

use std::sync::Arc;

use acton_reactive::ipc::{
    IpcAccessDenied, IpcAdmission, IpcConnectionContext, IpcConnectionInfo, IpcEnvelope,
    IpcIdentity, IpcOperation, IpcSecurityPolicy,
};
use tracing::{debug, warn};

use crate::declarations::{
    Checked, DeclarationTable, Enforcement, EnforcementMode, Operation, RejectionReport,
};
use crate::ipc_identity::{ConnectionIdentity, IdentityResolver};

/// What the policy saw.
///
/// Every method has a default no-op body, so a new hook never breaks an
/// implementor. Called from `admit` and `authorize`: an implementation MUST NOT
/// block or await, and should hand work off (a channel send, an atomic, a
/// `Notify`) rather than do it inline.
pub trait PolicyObserver: Send + Sync + std::panic::RefUnwindSafe + 'static {
    /// A connection was admitted and bound to this identity.
    fn on_admitted(&self, _identity: &ConnectionIdentity, _connection_id: usize) {}

    /// A `Subscribe` or `SubscribePatterns` was allowed, with these topics.
    ///
    /// Allowed is not the same as registered, and the difference is not the
    /// same on both paths. acton applies a plain subscribe immediately after
    /// authorizing it (`listener.rs:1570` then `:1592`, nothing in between),
    /// so there the hook means the subscription exists. A pattern subscribe is
    /// authorized at `:1497` and registered at `:1505` behind
    /// `rate_limiter.try_acquire()` at `:1502`, so a pattern batch refused by
    /// the rate limiter fires this hook and never registers. Treat the hook as
    /// "the engine permitted this", and do not let anything that must be
    /// correct wait on it without a deadline of its own.
    fn on_subscribed(&self, _identity: &ConnectionIdentity, _topics: &[String]) {}

    /// An operation fell outside the declarations, in warn or strict mode.
    fn on_violation(&self, _identity: &ConnectionIdentity, _reason: &str) {}
}

/// An observer that does nothing, used when nobody registered one.
struct NoObserver;
impl PolicyObserver for NoObserver {}

/// Whether the engine needs to install a policy at all (pure function).
///
/// Enforcement needs one. So does an observer, which is how another part of
/// the engine can watch admissions and subscriptions with enforcement off.
/// When neither applies the engine starts its listener the way it always did,
/// so the default configuration pays nothing, including on acton's per
/// notification [`IpcOperation::Deliver`] path.
#[must_use]
pub const fn policy_is_needed(mode: EnforcementMode, has_observer: bool) -> bool {
    mode.is_enforcing() || has_observer
}

/// The name to hold a connection to, and whether the engine vouches for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveName<'a> {
    /// The primitive name to check declarations against, if there is one.
    pub name: Option<&'a str>,
    /// Whether that name came from the engine rather than from the client.
    pub trusted: bool,
}

/// Decide which name enforcement applies to (pure function).
///
/// A managed connection is checked against the name the engine gave it, and the
/// `source` the client wrote is then a claim that has to match. An unmanaged
/// connection has no engine-given name, so the claim is all there is; it is
/// used, and it is not trusted. An empty `source` is not a claim.
#[must_use]
pub fn effective_name<'a>(
    identity: &'a ConnectionIdentity,
    claimed_source: Option<&'a str>,
) -> EffectiveName<'a> {
    match identity {
        ConnectionIdentity::Primitive { name } => EffectiveName {
            name: Some(name),
            trusted: true,
        },
        ConnectionIdentity::Unmanaged { .. } => EffectiveName {
            name: claimed_source,
            trusted: false,
        },
    }
}

/// Holds every connection to what its primitive declared.
pub struct EnginePolicy {
    declarations: Arc<DeclarationTable>,
    resolver: Arc<dyn IdentityResolver>,
    observer: Arc<dyn PolicyObserver>,
    /// Where a refusal goes to be turned into `system.error.<name>`.
    ///
    /// `authorize` runs on acton's connection task and must not block, and the
    /// subscription manager it would need does not exist until the listener has
    /// started, so the policy hands the report over and returns.
    rejections: Option<tokio::sync::mpsc::UnboundedSender<RejectionReport>>,
}

// A policy's own state is fixed once it is built: the declaration table, the
// resolver, the observer and the channel are never mutated, and `authorize`
// takes `&self`. Nothing the policy owns can be left half written by a panic,
// so observing it after one is sound.
impl std::panic::RefUnwindSafe for EnginePolicy {}

impl EnginePolicy {
    /// Build a policy over the declarations and an identity resolver.
    #[must_use]
    pub fn new(
        declarations: Arc<DeclarationTable>,
        resolver: Arc<dyn IdentityResolver>,
        observer: Option<Arc<dyn PolicyObserver>>,
    ) -> Self {
        Self {
            declarations,
            resolver,
            observer: observer.unwrap_or_else(|| Arc::new(NoObserver)),
            rejections: None,
        }
    }

    /// Send every refusal to this channel, to be reported as an event.
    #[must_use]
    pub fn reporting_to(
        mut self,
        rejections: tokio::sync::mpsc::UnboundedSender<RejectionReport>,
    ) -> Self {
        self.rejections = Some(rejections);
        self
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

    /// Log a violation, tell the observer, and report it if it was refused.
    fn record(
        &self,
        identity: &ConnectionIdentity,
        checked: Checked,
        name: Option<&str>,
        operation: Operation,
        message_type: &str,
    ) -> Result<(), IpcAccessDenied> {
        let report = RejectionReport::new(
            checked.verdict,
            name.unwrap_or(&identity.to_string()),
            operation,
            message_type,
            self.declarations.mode(),
        );
        warn!(
            primitive = %report.primitive,
            identity = %identity,
            identity.trusted = identity.is_managed(),
            operation = operation.as_str(),
            message.type = %message_type,
            mode = %self.declarations.mode(),
            "Declaration violation: {}",
            report.reason
        );
        self.observer.on_violation(identity, &report.reason);

        match checked.enforcement {
            Enforcement::Accept | Enforcement::Warn => Ok(()),
            Enforcement::Reject => {
                // `system.error.<name>` is attributed to a primitive. A
                // refusal that could not be tied to one has no name to
                // attribute the event to, so it stays in the log above.
                if let (Some(rejections), true) = (self.rejections.as_ref(), name.is_some())
                    && rejections.send(report.clone()).is_err()
                {
                    debug!("Rejection report dropped: the reporting task has stopped");
                }
                Err(IpcAccessDenied::new(report.reason))
            }
        }
    }

    /// Check one publish request.
    fn authorize_request(
        &self,
        identity: &ConnectionIdentity,
        envelope: &IpcEnvelope,
    ) -> Result<(), IpcAccessDenied> {
        // Anything that is not an Emergent publish is not a declaration
        // question: acton's own traffic passes straight through.
        let Some(message_type) = Self::published_type(envelope) else {
            return Ok(());
        };
        let claimed = Self::claimed_source(envelope);
        let effective = effective_name(identity, claimed);

        // A name the engine established is one the message may not contradict.
        // On an unmanaged connection the claim IS the name, so there is
        // nothing to contradict and nothing this could catch.
        if effective.trusted {
            let source_check = self.declarations.check_source(effective.name, claimed);
            if source_check.verdict.is_violation() {
                self.record(
                    identity,
                    source_check,
                    effective.name,
                    Operation::Publish,
                    message_type,
                )?;
            }
        }

        let checked = self
            .declarations
            .check(effective.name, Operation::Publish, message_type);
        if checked.verdict.is_violation() {
            self.record(
                identity,
                checked,
                effective.name,
                Operation::Publish,
                message_type,
            )?;
        }
        Ok(())
    }

    /// Check one batch of subscription topics.
    ///
    /// acton applies a batch atomically, so the batch is judged as a whole: the
    /// first topic outside the declarations denies all of them and nothing is
    /// applied. The denial names that topic so the operator knows which.
    fn authorize_subscribe(
        &self,
        identity: &ConnectionIdentity,
        topics: &[String],
    ) -> Result<(), IpcAccessDenied> {
        // A subscribe frame carries no `source`, so unlike a publish there is
        // no claim to fall back to. While the stub resolver is installed every
        // connection lands here, which is why subscribe enforcement is dormant
        // and says so at startup. Refusing instead would kill every handler
        // and sink the moment strict mode was turned on, for want of an
        // identity the engine has not learned how to establish yet.
        let effective = effective_name(identity, None);
        let Some(name) = effective.name else {
            debug!(
                identity = %identity,
                topics = topics.len(),
                "Subscription not checked: the connection has no name to check it against"
            );
            self.observer.on_subscribed(identity, topics);
            return Ok(());
        };
        for topic in topics {
            let checked = self
                .declarations
                .check(Some(name), Operation::Subscribe, topic);
            if checked.verdict.is_violation() {
                self.record(identity, checked, Some(name), Operation::Subscribe, topic)?;
            }
        }
        self.observer.on_subscribed(identity, topics);
        Ok(())
    }
}

impl IpcSecurityPolicy for EnginePolicy {
    fn admit(&self, connection: IpcConnectionInfo) -> IpcAdmission<'_> {
        Box::pin(async move {
            let identity = self.resolver.resolve(connection.peer_credentials()).await?;
            debug!(
                identity = %identity,
                identity.trusted = identity.is_managed(),
                peer.pid = ?connection.peer_credentials().and_then(|p| p.pid()),
                connection = connection.connection_id(),
                "Admitted an IPC connection"
            );
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
        // A connection with no identity of ours cannot have been admitted by
        // this policy. Treating it as unmanaged is the same answer admission
        // would have given it.
        let unidentified = ConnectionIdentity::Unmanaged { pid: None, uid: 0 };
        let identity = context
            .identity::<ConnectionIdentity>()
            .unwrap_or(&unidentified);

        match operation {
            IpcOperation::Request(envelope) => self.authorize_request(identity, envelope),
            IpcOperation::Subscribe(topics) | IpcOperation::SubscribePatterns(topics) => {
                self.authorize_subscribe(identity, topics)
            }
            // Dropping a subscription, asking what exists, and receiving a
            // notification are not declaration questions. Delivery in
            // particular is evaluated per notification per subscriber on the
            // broker's own task, and is redundant once subscribing is bound,
            // so it stays a single match arm and no work.
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
    use crate::declarations::Declarations;
    use crate::ipc_identity::StubResolver;
    use crate::primitives::PrimitiveKind;
    use std::sync::Mutex;

    fn managed(name: &str) -> ConnectionIdentity {
        ConnectionIdentity::Primitive {
            name: name.to_string(),
        }
    }

    fn unmanaged() -> ConnectionIdentity {
        ConnectionIdentity::Unmanaged {
            pid: Some(4242),
            uid: 1000,
        }
    }

    /// Records every violation the policy reported.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    impl Recorder {
        fn seen(&self) -> Vec<String> {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    impl PolicyObserver for Recorder {
        fn on_violation(&self, _identity: &ConnectionIdentity, reason: &str) {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(reason.to_string());
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
            Arc::new(StubResolver),
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

    // ========================================================================
    // Which name enforcement applies to
    // ========================================================================

    #[test]
    fn a_managed_connection_is_held_to_the_name_the_engine_gave_it() {
        let identity = managed("timer");
        assert_eq!(
            effective_name(&identity, Some("console")),
            EffectiveName {
                name: Some("timer"),
                trusted: true
            },
            "a claim does not override the engine's own answer"
        );
        assert_eq!(
            effective_name(&identity, None),
            EffectiveName {
                name: Some("timer"),
                trusted: true
            }
        );
    }

    #[test]
    fn an_unmanaged_connection_falls_back_to_the_name_it_claims() {
        let identity = unmanaged();
        assert_eq!(
            effective_name(&identity, Some("timer")),
            EffectiveName {
                name: Some("timer"),
                trusted: false
            },
            "the claim is used, and it is not trusted"
        );
        assert_eq!(
            effective_name(&identity, None),
            EffectiveName {
                name: None,
                trusted: false
            },
            "claiming nothing leaves nothing to check against"
        );
    }

    // ========================================================================
    // Publishing
    // ========================================================================

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
            for identity in [managed("timer"), unmanaged()] {
                assert!(
                    policy
                        .authorize_request(&identity, &publish("timer", "timer.tick"))
                        .is_ok(),
                    "mode {mode}, identity {identity}"
                );
            }
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
            let decision =
                policy.authorize_request(&managed("timer"), &publish("timer", "timer.undeclared"));
            assert_eq!(decision.is_ok(), allowed, "mode {mode}");
            assert_eq!(recorder.seen().len(), reports, "mode {mode}");
        }
    }

    #[test]
    fn an_undeclared_publish_is_caught_on_an_unmanaged_connection_too() {
        // The fallback is what makes enforcement useful before #24 lands: a
        // primitive naming itself honestly still cannot publish what it never
        // declared.
        let (policy, _) = policy(EnforcementMode::Strict);
        let denied = denial(
            policy.authorize_request(&unmanaged(), &publish("timer", "timer.undeclared")),
            "an undeclared publish is refused whatever the connection",
        );
        assert_eq!(
            denied,
            "'timer' tried to publish 'timer.undeclared', which is not in its declared publishes list"
        );
    }

    #[test]
    fn a_managed_publish_may_not_name_a_source_other_than_its_own_connection() {
        let (policy, _) = policy(EnforcementMode::Strict);
        let denied = denial(
            policy.authorize_request(&managed("timer"), &publish("console", "timer.tick")),
            "a spoofed source is refused on a managed connection",
        );
        assert!(denied.contains("named a different source"), "{denied}");
    }

    #[test]
    fn an_unmanaged_publish_can_still_claim_any_name_it_likes() {
        // The documented gap that #24 closes, stated as a test so that it
        // fails the day a real resolver starts naming connections.
        let (policy, _) = policy(EnforcementMode::Strict);
        assert!(
            policy
                .authorize_request(&unmanaged(), &publish("timer", "timer.tick"))
                .is_ok(),
            "an unmanaged connection publishing as 'timer' is accepted today"
        );
    }

    #[test]
    fn a_publish_claiming_a_primitive_that_is_not_configured_is_refused() {
        let (policy, _) = policy(EnforcementMode::Strict);
        let denied = denial(
            policy.authorize_request(&unmanaged(), &publish("intruder", "anything.at.all")),
            "an unconfigured name is refused",
        );
        assert!(
            denied.contains("no primitive of that name is configured"),
            "{denied}"
        );
    }

    #[test]
    fn a_publish_claiming_nothing_at_all_is_refused_in_strict() {
        let (policy, _) = policy(EnforcementMode::Strict);
        let denied = denial(
            policy.authorize_request(&unmanaged(), &publish("", "anything.at.all")),
            "a connection offering no name is refused",
        );
        assert!(denied.contains("could not tie"), "{denied}");
    }

    #[test]
    fn a_request_that_is_not_an_emergent_publish_is_not_a_declaration_question() {
        let (policy, recorder) = policy(EnforcementMode::Strict);
        let envelope = IpcEnvelope::new(
            "message_broker",
            "SystemEvent",
            serde_json::json!({ "event": "started" }),
        );
        assert!(
            policy
                .authorize_request(&managed("timer"), &envelope)
                .is_ok()
        );
        assert!(recorder.seen().is_empty());
    }

    #[test]
    fn an_unidentified_connection_may_still_ask_the_engine_a_protocol_question() {
        let (policy, recorder) = policy(EnforcementMode::Strict);
        assert!(
            policy
                .authorize_request(&unmanaged(), &publish("", "system.request.topology"))
                .is_ok(),
            "a CLI or topology viewer keeps working under strict"
        );
        assert!(recorder.seen().is_empty());
    }

    // ========================================================================
    // Subscribing
    // ========================================================================

    #[test]
    fn a_subscribe_batch_is_refused_whole_when_one_topic_is_undeclared() {
        let (policy, recorder) = policy(EnforcementMode::Strict);
        let identity = managed("console");
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
        assert_eq!(recorder.seen().len(), 1);
    }

    #[test]
    fn a_source_may_not_subscribe_at_all() {
        let (policy, _) = policy(EnforcementMode::Strict);
        let denied = denial(
            policy.authorize_subscribe(&managed("timer"), &["timer.tick".to_owned()]),
            "a source has nothing it may subscribe to",
        );
        assert!(denied.contains("its kind cannot subscribe"), "{denied}");
    }

    #[test]
    fn an_unmanaged_connection_has_its_subscriptions_let_through_unchecked() {
        // The documented consequence of the stub resolver, stated as a test so
        // that it fails the day #24 starts naming connections and subscribe
        // enforcement turns on by itself.
        let (policy, recorder) = policy(EnforcementMode::Strict);
        assert!(
            policy
                .authorize_subscribe(&unmanaged(), &["secrets.all".to_owned()])
                .is_ok(),
            "there is no name on a subscribe frame to check the topic against"
        );
        assert!(recorder.seen().is_empty());
    }

    #[test]
    fn a_subscription_the_engine_could_not_check_still_reaches_the_observer() {
        // Issue #66 uses on_subscribed as its readiness signal, so it has to
        // fire whether or not the topics could be judged.
        #[derive(Default)]
        struct Ready(Mutex<Vec<String>>);
        impl PolicyObserver for Ready {
            fn on_subscribed(&self, _identity: &ConnectionIdentity, topics: &[String]) {
                self.0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(topics);
            }
        }
        let ready = Arc::new(Ready::default());
        let policy = EnginePolicy::new(
            declarations(EnforcementMode::Strict),
            Arc::new(StubResolver),
            Some(ready.clone()),
        );
        assert!(
            policy
                .authorize_subscribe(&unmanaged(), &["timer.tick".to_owned()])
                .is_ok()
        );
        assert_eq!(
            ready.0.lock().unwrap_or_else(|e| e.into_inner()).as_slice(),
            ["timer.tick"]
        );
    }

    // ========================================================================
    // Everything else acton can ask
    // ========================================================================

    #[test]
    fn a_rejection_that_names_nobody_is_logged_and_not_reported_as_an_event() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let policy = EnginePolicy::new(
            declarations(EnforcementMode::Strict),
            Arc::new(StubResolver),
            None,
        )
        .reporting_to(tx);

        // Named: reported, so a sink on system.error.* sees it.
        assert!(
            policy
                .authorize_request(&unmanaged(), &publish("timer", "timer.undeclared"))
                .is_err()
        );
        match rx.try_recv() {
            Ok(reported) => assert_eq!(reported.primitive, "timer"),
            Err(e) => panic!("a named refusal is reported, not dropped: {e}"),
        }

        // Unnamed: there is no primitive to attribute an event to.
        assert!(
            policy
                .authorize_request(&unmanaged(), &publish("", "anything.at.all"))
                .is_err()
        );
        assert!(
            rx.try_recv().is_err(),
            "a refusal that names nobody stays in the log"
        );
    }
}
