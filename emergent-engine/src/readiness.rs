//! Deciding when a startup tier has had long enough, and who is holding it up.
//!
//! The engine starts primitives in tiers (sinks, then handlers, then sources)
//! so that consumers are listening before producers run. That only works if the
//! engine waits for the tier it just started, and the only thing it can wait on
//! is what it can actually observe.
//!
//! # The signal acton-reactive 9.4.0 added
//!
//! Line references below are against 9.4.1, the version this workspace pins.
//!
//! acton takes an optional `IpcSecurityPolicy` and calls its `authorize` with
//! `IpcOperation::Subscribe(&request.message_types)` at `listener.rs:1570`,
//! immediately before `ctx.subscription_manager.subscribe(conn_id, ...)` at
//! `listener.rs:1592`. Nothing is awaited between the two, so a subscribe the
//! policy authorized is a subscribe that registers: this is the "subscription
//! confirmed" signal 9.3.0 did not have, and it needs no settle window. The
//! pattern form authorizes at `listener.rs:1497` and registers at
//! `listener.rs:1505`, but behind `rate_limiter.try_acquire()`, so an
//! authorized pattern subscribe can still be rate limited away; the deadline
//! below is what covers that.
//!
//! `authorize` (`listener.rs:938-951`) only reaches the policy when a policy is
//! installed and the connection has an authenticated context, and returns
//! `Ok(())` otherwise, so this signal exists only once the engine's policy is
//! wired in.
//!
//! The policy names the subscriber with a `ConnectionIdentity`. Until
//! Govcraft/emergent#24 teaches the engine to resolve a peer to a primitive,
//! every identity is `Unmanaged`, carrying the kernel's peer credentials and
//! nothing else. [`attribute`] joins the pid in those credentials against the
//! pids of the children the engine spawned, which is exact for a primitive the
//! engine launched directly. A peer with no pid, or a pid belonging to a
//! descendant rather than the spawned process (a `uv` launcher's `python3`),
//! stays unattributed and falls through to the deadline. acton reports the pid
//! through tokio's `UnixStream::peer_cred` (`listener.rs:782-794`) and treats a
//! platform that declines to report one as `None`
//! (`subscription_manager.rs:161-167`), so the join is Linux-solid and
//! degrades to the deadline elsewhere.
//!
//! [`StartupObserver::note_subscribe`] is called from inside acton's
//! connection task. It filters and forwards on an unbounded channel, and does
//! nothing else, because blocking there would stall the very subscribe it is
//! reporting.
//!
//! # What this replaced
//!
//! acton-reactive 9.3.0 had no client name anywhere: no sender on an
//! `IpcEnvelope`, no name on a `ConnectionInfo`, and no connect, disconnect or
//! subscribe callback. Until the policy hook the engine inferred readiness from
//! IPC traffic carrying a primitive's own `source` name, plus a settle window
//! standing in for the subscribe it could not see, joined with a probe over
//! `1..=connections_accepted()` that leaned on `ConnectionId` being the running
//! accept count. Both were bets on an implementation detail, and the first one
//! missed a primitive that passes its topics in code rather than asking the
//! engine for them. `on_subscribed` replaced both.
//!
//! # Why only startup
//!
//! A restart-policy respawn is one primitive coming back inside its own actor,
//! with no tier behind it. The rest of the topology is already running and
//! publishing whether or not it has re-subscribed, the events lost between the
//! crash and the respawn are gone either way, and holding them would mean
//! pausing every other primitive for this one, which the engine has no way to
//! do. So a restart gets no wait. Startup is the only place where the engine
//! controls what happens next and can usefully hold it.
//!
//! [`evaluate_tier`], [`classify`], [`attribute`], [`absorb`] and
//! [`is_primitive_subscription`] are pure. [`StartupObserver`] and the polling
//! loop in [`crate::process_manager`] gather the observations; these decide.

use std::collections::{BTreeSet, HashMap};

use tokio::sync::{Mutex, mpsc};

use crate::ipc_identity::ConnectionIdentity;
use crate::ipc_policy::PolicyObserver;

/// The topic the SDK's throwaway discovery connection subscribes to.
///
/// A connection holding only this is a primitive in the middle of asking for
/// its configured subscriptions, not one that has subscribed.
pub const DISCOVERY_RESPONSE_TOPIC: &str = "system.response.subscriptions";

/// What the engine has observed about one primitive while waiting on its tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// The policy authorized a subscribe from this primitive.
    Subscribed,
    /// Nothing yet, and the child is still running, so it may be about to.
    Waiting,
    /// The child is not running: it exited, failed, or never spawned. It cannot
    /// become ready, so it must not hold the tier.
    NotRunning,
}

/// Whether a connection's exact subscriptions are a primitive's real
/// subscription rather than the SDK's throwaway discovery connection (pure).
///
/// Every SDK adds `system.shutdown` to a real subscribe, so a real
/// subscription always carries something the discovery connection does not.
#[must_use]
pub fn is_primitive_subscription(subscribed_types: &[String]) -> bool {
    subscribed_types
        .iter()
        .any(|t| t != DISCOVERY_RESPONSE_TOPIC)
}

/// Classify one primitive from what the engine observed (pure).
///
/// `subscription_confirmed` outranks liveness: a primitive whose subscribe was
/// authorized is ready even if its process has since gone, because whatever it
/// subscribed to is registered against a connection the tier no longer needs to
/// wait for.
#[must_use]
pub const fn classify(running: bool, subscription_confirmed: bool) -> Evidence {
    if subscription_confirmed {
        Evidence::Subscribed
    } else if running {
        Evidence::Waiting
    } else {
        Evidence::NotRunning
    }
}

/// One primitive of the tier the engine is waiting on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierMember {
    /// The primitive's configured name.
    pub name: String,
    /// Whether it declares any `subscribes`. One that declares none has nothing
    /// to be ready for.
    pub declares_subscriptions: bool,
    /// What the engine observed about it.
    pub evidence: Evidence,
}

/// Whether the tier may be left behind, and who is still holding it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierVerdict {
    /// True when nothing is left to wait for.
    pub ready: bool,
    /// Names of the primitives still being waited on, sorted. Empty when
    /// `ready`. This is what the deadline warning lists.
    pub waiting_on: Vec<String>,
}

/// Decide whether a tier is ready and name whoever is not (pure).
///
/// A member holds the tier only if it declares subscriptions and its evidence
/// is still [`Evidence::Waiting`]. Everything else,
/// including a primitive that failed to spawn or exited during the wait, is out
/// of the way.
#[must_use]
pub fn evaluate_tier(members: &[TierMember]) -> TierVerdict {
    let mut waiting_on: Vec<String> = members
        .iter()
        .filter(|m| m.declares_subscriptions)
        .filter(|m| matches!(m.evidence, Evidence::Waiting))
        .map(|m| m.name.clone())
        .collect();
    waiting_on.sort();
    TierVerdict {
        ready: waiting_on.is_empty(),
        waiting_on,
    }
}

// ---------------------------------------------------------------------------
// The 9.4.0 signal: a SUBSCRIBE the policy authorized
// ---------------------------------------------------------------------------

/// Who the policy said was subscribing, reduced to what readiness can act on.
///
/// The policy hands the observer a `ConnectionIdentity`; this is that enum with
/// everything readiness does not use dropped, so the decision below stays free
/// of the identity module and can be table tested on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservedSubscriber {
    /// The policy named the primitive, so no inference is needed.
    Named(String),
    /// The policy had only the kernel's peer credentials, and they carried a
    /// pid. Every connection looks like this until Govcraft/emergent#24 lands.
    Pid(u32),
    /// Neither a name nor a pid. Nothing readiness can attribute.
    Anonymous,
}

/// Name the primitive behind an observed subscribe (pure).
///
/// `children` maps the pid of each process the engine spawned to its configured
/// name. A pid that is not in it belongs to something the engine did not spawn,
/// or to a descendant of something it did (a `uv` launcher's `python3`, say),
/// and cannot be named until #24 resolves ancestry.
#[must_use]
pub fn attribute(
    subscriber: &ObservedSubscriber,
    children: &HashMap<u32, String>,
) -> Option<String> {
    match subscriber {
        ObservedSubscriber::Named(name) => Some(name.clone()),
        ObservedSubscriber::Pid(pid) => children.get(pid).cloned(),
        ObservedSubscriber::Anonymous => None,
    }
}

/// How many unattributable observations are kept for a later retry.
///
/// Only a subscribe whose pid the engine does not yet recognise lands here, and
/// only during startup, so the cap is a guard against a pathological client
/// rather than a working limit.
pub const UNATTRIBUTED_CAP: usize = 64;

/// Every subscribe readiness has been told about, folded into names.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Ledger {
    /// Primitives whose subscribe the policy authorized and readiness could
    /// name. Sorted, because it is logged.
    pub confirmed: BTreeSet<String>,
    /// Observations that named nobody, kept in case the pid becomes known.
    pub unattributed: Vec<ObservedSubscriber>,
}

/// Fold new observations into the ledger (pure).
///
/// Unattributable observations are retried on the next fold: a child's pid is
/// recorded when it is spawned and a subscribe can only follow that, but a
/// restart can reorder the two, and retrying costs a map lookup.
#[must_use]
pub fn absorb(
    ledger: Ledger,
    observed: impl IntoIterator<Item = ObservedSubscriber>,
    children: &HashMap<u32, String>,
) -> Ledger {
    let Ledger {
        mut confirmed,
        unattributed,
    } = ledger;
    let mut still_unattributed = Vec::new();
    for subscriber in unattributed.into_iter().chain(observed) {
        match attribute(&subscriber, children) {
            Some(name) => {
                confirmed.insert(name);
            }
            None if still_unattributed.len() < UNATTRIBUTED_CAP => {
                still_unattributed.push(subscriber);
            }
            None => {}
        }
    }
    Ledger {
        confirmed,
        unattributed: still_unattributed,
    }
}

/// The sending half of the readiness signal, handed to the engine's policy.
///
/// `note_subscribe` runs inside acton's connection task, so it does no work
/// beyond one cheap filter and an unbounded send, which never blocks and never
/// awaits.
#[derive(Debug, Clone)]
pub struct StartupObserver {
    tx: mpsc::UnboundedSender<ObservedSubscriber>,
}

/// The receiving half, drained by the startup wait.
#[derive(Debug)]
pub struct SubscribeSignals {
    rx: Mutex<mpsc::UnboundedReceiver<ObservedSubscriber>>,
}

impl StartupObserver {
    /// Build both halves of the signal.
    #[must_use]
    pub fn channel() -> (Self, SubscribeSignals) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, SubscribeSignals { rx: Mutex::new(rx) })
    }

    /// Record an authorized subscribe. Called from the policy; must not block.
    ///
    /// A connection holding nothing but the discovery topic is the SDK asking
    /// what it is configured to subscribe to, not a primitive that has
    /// subscribed, so it is dropped here rather than travelling.
    pub fn note_subscribe(&self, subscriber: ObservedSubscriber, topics: &[String]) {
        if !is_primitive_subscription(topics) {
            return;
        }
        // The receiver lives as long as the startup wait. After it, sends fail
        // and are meant to: nothing is waiting on them.
        let _ = self.tx.send(subscriber);
    }
}

impl SubscribeSignals {
    /// Take everything observed since the last drain.
    pub async fn drain(&self) -> Vec<ObservedSubscriber> {
        let mut rx = self.rx.lock().await;
        let mut observed = Vec::new();
        while let Ok(subscriber) = rx.try_recv() {
            observed.push(subscriber);
        }
        observed
    }
}

/// Reduce a policy identity to what readiness can act on (pure).
///
/// The name is used directly when the engine established one. Otherwise the
/// peer's pid is all there is, and [`attribute`] joins it against the children
/// the engine spawned. Before Govcraft/emergent#24 every identity arrives as
/// `Unmanaged`, so the pid is the only path in practice.
#[must_use]
pub fn observed_subscriber(identity: &ConnectionIdentity) -> ObservedSubscriber {
    match identity {
        ConnectionIdentity::Primitive { name } => ObservedSubscriber::Named(name.clone()),
        ConnectionIdentity::Unmanaged { pid: Some(pid), .. } => ObservedSubscriber::Pid(*pid),
        ConnectionIdentity::Unmanaged { pid: None, .. } => ObservedSubscriber::Anonymous,
    }
}

impl PolicyObserver for StartupObserver {
    fn on_subscribed(&self, identity: &ConnectionIdentity, topics: &[String]) {
        self.note_subscribe(observed_subscriber(identity), topics);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, declares_subscriptions: bool, evidence: Evidence) -> TierMember {
        TierMember {
            name: name.to_string(),
            declares_subscriptions,
            evidence,
        }
    }

    #[test]
    fn liveness_and_the_subscribe_signal_map_to_evidence() {
        let cases = [
            (
                "not heard from, still running: keep waiting",
                true,
                false,
                Evidence::Waiting,
            ),
            (
                "not heard from and gone: nothing to wait for",
                false,
                false,
                Evidence::NotRunning,
            ),
            (
                "subscribed and running: ready",
                true,
                true,
                Evidence::Subscribed,
            ),
            (
                "subscribed and then exited: still ready, the subscribe happened",
                false,
                true,
                Evidence::Subscribed,
            ),
        ];
        for (case, running, confirmed, expected) in cases {
            assert_eq!(classify(running, confirmed), expected, "{case}");
        }
    }

    #[test]
    fn a_tier_is_ready_once_nobody_is_left_to_wait_for() {
        struct Case {
            name: &'static str,
            members: Vec<TierMember>,
            ready: bool,
            waiting_on: Vec<&'static str>,
        }

        let cases = vec![
            Case {
                name: "an empty tier is ready",
                members: vec![],
                ready: true,
                waiting_on: vec![],
            },
            Case {
                name: "a tier of sources, which subscribe to nothing, never waits",
                members: vec![
                    member("timer", false, Evidence::Waiting),
                    member("webhook", false, Evidence::Waiting),
                ],
                ready: true,
                waiting_on: vec![],
            },
            Case {
                name: "one silent subscriber holds the tier",
                members: vec![
                    member("console", true, Evidence::Subscribed),
                    member("viewer", true, Evidence::Waiting),
                ],
                ready: false,
                waiting_on: vec!["viewer"],
            },
            Case {
                name: "in contact is not yet ready",
                members: vec![member("console", true, Evidence::Waiting)],
                ready: false,
                waiting_on: vec!["console"],
            },
            Case {
                name: "everyone settled",
                members: vec![
                    member("console", true, Evidence::Subscribed),
                    member("log", true, Evidence::Subscribed),
                ],
                ready: true,
                waiting_on: vec![],
            },
            Case {
                name: "a primitive that failed to spawn does not hang the tier",
                members: vec![
                    member("console", true, Evidence::Subscribed),
                    member("broken", true, Evidence::NotRunning),
                ],
                ready: true,
                waiting_on: vec![],
            },
            Case {
                name: "waiting_on is sorted, whatever order the tier came in",
                members: vec![
                    member("zulu", true, Evidence::Waiting),
                    member("alpha", true, Evidence::Waiting),
                    member("mike", true, Evidence::Waiting),
                ],
                ready: false,
                waiting_on: vec!["alpha", "mike", "zulu"],
            },
            Case {
                name: "a confirmed subscription releases the tier",
                members: vec![
                    member("console", true, Evidence::Subscribed),
                    member("filter", true, Evidence::Subscribed),
                ],
                ready: true,
                waiting_on: vec![],
            },
            Case {
                name: "a subscriber-less primitive is ignored even while silent",
                members: vec![
                    member("timer", false, Evidence::Waiting),
                    member("console", true, Evidence::Subscribed),
                ],
                ready: true,
                waiting_on: vec![],
            },
        ];

        for case in cases {
            let verdict = evaluate_tier(&case.members);
            assert_eq!(verdict.ready, case.ready, "ready, case: {}", case.name);
            assert_eq!(
                verdict.waiting_on,
                case.waiting_on
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect::<Vec<_>>(),
                "waiting_on, case: {}",
                case.name
            );
        }
    }

    #[test]
    fn a_discovery_only_connection_is_not_a_subscription() {
        let cases: Vec<(&str, Vec<&str>, bool)> = vec![
            ("a connection with nothing subscribed", vec![], false),
            (
                "the SDK's discovery connection alone",
                vec![DISCOVERY_RESPONSE_TOPIC],
                false,
            ),
            (
                "a real subscribe, which always carries system.shutdown",
                vec!["timer.tick", "system.shutdown"],
                true,
            ),
            (
                "system.shutdown on its own is still a real subscribe",
                vec!["system.shutdown"],
                true,
            ),
            (
                "a primitive that also wants the discovery topic",
                vec![DISCOVERY_RESPONSE_TOPIC, "system.shutdown"],
                true,
            ),
        ];

        for (name, types, want) in cases {
            let owned: Vec<String> = types.iter().map(|t| (*t).to_string()).collect();
            assert_eq!(is_primitive_subscription(&owned), want, "case: {name}");
        }
    }

    /// `ready` and `waiting_on` are two views of one decision and cannot
    /// disagree.
    #[test]
    fn ready_is_exactly_an_empty_waiting_list() {
        for evidence in [
            Evidence::Waiting,
            Evidence::Waiting,
            Evidence::Subscribed,
            Evidence::Subscribed,
            Evidence::NotRunning,
        ] {
            for declares in [true, false] {
                let verdict = evaluate_tier(&[member("p", declares, evidence)]);
                assert_eq!(verdict.ready, verdict.waiting_on.is_empty());
            }
        }
    }

    fn children(pairs: &[(u32, &str)]) -> HashMap<u32, String> {
        pairs
            .iter()
            .map(|(pid, name)| (*pid, (*name).to_string()))
            .collect()
    }

    #[test]
    fn a_subscriber_is_named_by_the_policy_or_by_its_pid() {
        let spawned = children(&[(41, "console"), (42, "log")]);
        let cases = [
            (
                "the policy already knew the name",
                ObservedSubscriber::Named("console".to_string()),
                Some("console"),
            ),
            (
                "a spawned child's pid names it",
                ObservedSubscriber::Pid(42),
                Some("log"),
            ),
            (
                "a pid the engine did not spawn names nobody",
                ObservedSubscriber::Pid(999),
                None,
            ),
            (
                "no name and no pid names nobody",
                ObservedSubscriber::Anonymous,
                None,
            ),
        ];
        for (case, subscriber, expected) in cases {
            assert_eq!(
                attribute(&subscriber, &spawned).as_deref(),
                expected,
                "{case}"
            );
        }
    }

    #[test]
    fn absorbing_observations_confirms_what_it_can_name_and_keeps_the_rest() {
        let spawned = children(&[(41, "console")]);
        let ledger = absorb(
            Ledger::default(),
            [
                ObservedSubscriber::Pid(41),
                ObservedSubscriber::Named("log".to_string()),
                ObservedSubscriber::Pid(77),
                ObservedSubscriber::Anonymous,
            ],
            &spawned,
        );
        assert_eq!(
            ledger.confirmed.iter().cloned().collect::<Vec<_>>(),
            vec!["console".to_string(), "log".to_string()],
            "both nameable subscribes are confirmed, in sorted order"
        );
        assert_eq!(
            ledger.unattributed,
            vec![ObservedSubscriber::Pid(77), ObservedSubscriber::Anonymous],
            "the rest is kept for a later fold"
        );
    }

    #[test]
    fn an_unattributed_observation_is_named_once_its_pid_is_known() {
        let early = absorb(
            Ledger::default(),
            [ObservedSubscriber::Pid(41)],
            &children(&[]),
        );
        assert!(early.confirmed.is_empty(), "nothing to name it with yet");

        let later = absorb(early, [], &children(&[(41, "console")]));
        assert_eq!(
            later.confirmed.iter().cloned().collect::<Vec<_>>(),
            vec!["console".to_string()],
            "the retry names it once the child is known"
        );
        assert!(later.unattributed.is_empty(), "and the backlog clears");
    }

    #[test]
    fn the_unattributed_backlog_is_capped() {
        let noise = (0..UNATTRIBUTED_CAP * 2).map(|_| ObservedSubscriber::Anonymous);
        let ledger = absorb(Ledger::default(), noise, &children(&[]));
        assert_eq!(ledger.unattributed.len(), UNATTRIBUTED_CAP);
    }

    #[tokio::test]
    async fn an_authorized_subscribe_reaches_the_startup_wait() {
        let (observer, signals) = StartupObserver::channel();
        observer.note_subscribe(
            ObservedSubscriber::Pid(41),
            &["timer.tick".to_string(), "system.shutdown".to_string()],
        );
        assert_eq!(signals.drain().await, vec![ObservedSubscriber::Pid(41)]);
        assert!(
            signals.drain().await.is_empty(),
            "a drained signal is not delivered twice"
        );
    }

    #[tokio::test]
    async fn a_discovery_only_subscribe_is_not_signalled() {
        let (observer, signals) = StartupObserver::channel();
        observer.note_subscribe(
            ObservedSubscriber::Pid(41),
            &[DISCOVERY_RESPONSE_TOPIC.to_string()],
        );
        observer.note_subscribe(ObservedSubscriber::Pid(41), &[]);
        assert!(
            signals.drain().await.is_empty(),
            "asking what to subscribe to is not subscribing"
        );
    }

    #[tokio::test]
    async fn an_observer_outliving_the_startup_wait_does_not_panic() {
        let (observer, signals) = StartupObserver::channel();
        drop(signals);
        observer.note_subscribe(ObservedSubscriber::Pid(41), &["timer.tick".to_string()]);
    }
}
