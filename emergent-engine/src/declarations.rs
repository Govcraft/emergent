//! Decide whether a primitive may publish a message type it did not declare.
//!
//! A primitive's `publishes` and `subscribes` lists in `emergent.toml` are the
//! topology. Until now they were advisory on the publish side: the engine
//! stored and forwarded whatever a client sent, so a typo in a `publishes`
//! entry produced a message that flowed anyway and a declaration that no longer
//! described the system.
//!
//! The decisions here are pure. [`DeclarationTable`] is built once from the
//! configuration and answers each publish with a set lookup plus a scan of the
//! primitive's wildcard prefixes, which are normally none.
//!
//! What this enforces, and what it does not: the key is the message's `source`
//! field, which the client fills in itself. Enforcement therefore catches an
//! undeclared topic from a primitive that names itself honestly, which is the
//! case every typo and every drifted declaration falls into. It does not catch
//! a client that puts another primitive's name in `source`, because the engine
//! has no connection identity to check the name against. Issue #24 tracks
//! authenticating connections by spawned PID, which is what would close that.

use std::collections::{HashMap, HashSet};

use emergent_client::{TopicKind, classify_topic, pattern_prefix};
use serde::{Deserialize, Serialize};

use crate::primitives::PrimitiveKind;

/// How the engine reacts to a primitive operating outside its declarations.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EnforcementMode {
    /// Declarations stay advisory. Nothing is checked and nothing is logged.
    #[default]
    Off,
    /// Violations are logged at WARN, and the message still flows.
    Warn,
    /// Violations are logged, rejected, and reported as `system.error.<name>`.
    Strict,
}

impl EnforcementMode {
    /// The name this mode is written under in `[engine].enforce_declarations`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Warn => "warn",
            Self::Strict => "strict",
        }
    }

    /// Whether the engine needs to check anything at all in this mode.
    #[must_use]
    pub const fn is_enforcing(&self) -> bool {
        !matches!(self, Self::Off)
    }
}

impl std::fmt::Display for EnforcementMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The operation being checked against a primitive's declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// The primitive sent a message for the broker to store and forward.
    Publish,
    /// The primitive asked to receive a message type.
    ///
    /// Reachable only if acton-reactive grows a hook on the SUBSCRIBE frame;
    /// see the module docs of `main.rs` and `docs/configuration.md`. The
    /// decision is written here so the rule exists in one place when it is.
    Subscribe,
}

impl Operation {
    /// The word used for this operation in logs and error text.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Publish => "publish",
            Self::Subscribe => "subscribe",
        }
    }

    /// The `emergent.toml` key holding the declarations this operation checks.
    #[must_use]
    pub const fn config_key(&self) -> &'static str {
        match self {
            Self::Publish => "publishes",
            Self::Subscribe => "subscribes",
        }
    }
}

/// The outcome of checking one operation against one primitive's declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The type appears in the primitive's declarations for this operation.
    Declared,
    /// The type is engine protocol, allowed to every connected primitive.
    Protocol,
    /// The primitive is declared, but not for this type.
    Undeclared,
    /// The primitive's kind cannot perform this operation at all.
    KindCannot,
    /// No primitive of this name is configured.
    UnknownPrimitive,
}

impl Verdict {
    /// Whether this verdict describes an operation outside the declarations.
    #[must_use]
    pub const fn is_violation(&self) -> bool {
        matches!(
            self,
            Self::Undeclared | Self::KindCannot | Self::UnknownPrimitive
        )
    }
}

/// What the engine does about a verdict in a given mode (pure function).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enforcement {
    /// Carry on: either the operation is fine, or enforcement is off.
    Accept,
    /// Log the violation and carry on.
    Warn,
    /// Log the violation, refuse the operation, and report it.
    Reject,
}

/// Combine a mode and a verdict into what the engine actually does.
#[must_use]
pub const fn enforcement_for(mode: EnforcementMode, verdict: Verdict) -> Enforcement {
    if !verdict.is_violation() {
        return Enforcement::Accept;
    }
    match mode {
        EnforcementMode::Off => Enforcement::Accept,
        EnforcementMode::Warn => Enforcement::Warn,
        EnforcementMode::Strict => Enforcement::Reject,
    }
}

/// Message types every connected primitive may publish whatever it declared.
///
/// These are how an SDK asks the engine a question over the same socket it
/// publishes on. They are protocol rather than topology: no primitive lists
/// them in `publishes`, every SDK sends the first one before it can subscribe
/// to anything, and the answers come from the engine, never from a primitive.
pub const PROTOCOL_TOPICS: [&str; 2] = ["system.request.subscriptions", "system.request.topology"];

/// Whether a message type is engine protocol rather than topology.
#[must_use]
pub fn is_protocol_topic(message_type: &str) -> bool {
    PROTOCOL_TOPICS.contains(&message_type)
}

/// A primitive's declared topics for one operation, prepared for lookup.
///
/// Exact topics, which is nearly all of them, answer from a hash set. Terminal
/// wildcards are kept as the prefix before the `*`, so `system.error.*` becomes
/// `system.error.` and the bare `*` becomes the empty string, which every type
/// starts with. A topic acton would never deliver (a misplaced wildcard) is
/// dropped here exactly as it is dropped there, so enforcement never permits
/// something the router would not carry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TopicSet {
    exact: HashSet<String>,
    prefixes: Vec<String>,
}

impl TopicSet {
    /// Build a lookup set from declared topics.
    #[must_use]
    pub fn new(topics: &[String]) -> Self {
        let mut set = Self::default();
        for topic in topics {
            match classify_topic(topic) {
                Ok(TopicKind::Exact) => {
                    set.exact.insert(topic.clone());
                }
                Ok(TopicKind::Pattern) => {
                    if let Some(prefix) = pattern_prefix(topic) {
                        set.prefixes.push(prefix.to_owned());
                    }
                }
                Err(_) => {}
            }
        }
        set
    }

    /// Whether these declarations cover a message type.
    #[must_use]
    pub fn permits(&self, message_type: &str) -> bool {
        self.exact.contains(message_type)
            || self
                .prefixes
                .iter()
                .any(|prefix| message_type.starts_with(prefix.as_str()))
    }

    /// Whether nothing was declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.prefixes.is_empty()
    }
}

/// One primitive's kind and prepared declarations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declarations {
    /// Which of the three primitives this is.
    pub kind: PrimitiveKind,
    /// Prepared `publishes` list.
    pub publishes: TopicSet,
    /// Prepared `subscribes` list.
    pub subscribes: TopicSet,
}

impl Declarations {
    /// Prepare one primitive's declared lists for lookup.
    #[must_use]
    pub fn new(kind: PrimitiveKind, publishes: &[String], subscribes: &[String]) -> Self {
        Self {
            kind,
            publishes: TopicSet::new(publishes),
            subscribes: TopicSet::new(subscribes),
        }
    }

    /// Whether this kind may perform an operation at all.
    #[must_use]
    pub const fn kind_permits(&self, operation: Operation) -> bool {
        !matches!(
            (self.kind, operation),
            (PrimitiveKind::Sink, Operation::Publish)
                | (PrimitiveKind::Source, Operation::Subscribe)
        )
    }

    /// The set governing one operation.
    #[must_use]
    pub const fn topics_for(&self, operation: Operation) -> &TopicSet {
        match operation {
            Operation::Publish => &self.publishes,
            Operation::Subscribe => &self.subscribes,
        }
    }
}

/// Decide one operation against one primitive's declarations (pure function).
///
/// `declarations` is `None` when no primitive of that name is configured.
/// Protocol topics are settled before anything else, so an unknown or
/// wrong-kind client can still ask the engine the questions every SDK asks.
#[must_use]
pub fn decide(
    declarations: Option<&Declarations>,
    operation: Operation,
    message_type: &str,
) -> Verdict {
    if is_protocol_topic(message_type) {
        return Verdict::Protocol;
    }
    let Some(declarations) = declarations else {
        return Verdict::UnknownPrimitive;
    };
    if !declarations.kind_permits(operation) {
        return Verdict::KindCannot;
    }
    if declarations.topics_for(operation).permits(message_type) {
        Verdict::Declared
    } else {
        Verdict::Undeclared
    }
}

/// The sentence the engine logs, replies with, and puts in `system.error.<name>`.
///
/// Written once so the log line, the event payload and anything a client is
/// told all say the same thing.
#[must_use]
pub fn violation_reason(
    verdict: Verdict,
    name: &str,
    operation: Operation,
    message_type: &str,
) -> String {
    let op = operation.as_str();
    let field = operation.config_key();
    match verdict {
        Verdict::UnknownPrimitive => format!(
            "'{name}' tried to {op} '{message_type}' but no primitive of that name is configured"
        ),
        Verdict::KindCannot => {
            format!("'{name}' tried to {op} '{message_type}' but its kind cannot {op}")
        }
        Verdict::Undeclared => format!(
            "'{name}' tried to {op} '{message_type}', which is not in its declared {field} list"
        ),
        Verdict::Declared | Verdict::Protocol => format!("'{name}' may {op} '{message_type}'"),
    }
}

/// The message type of the event reporting a rejected operation.
///
/// `system.error.<name>` is the type the engine already reports a primitive's
/// failures under, so a sink subscribed to `system.error.*` sees a rejection
/// without subscribing to anything new.
#[must_use]
pub fn rejection_event_type(name: &str) -> String {
    format!("system.error.{name}")
}

/// The payload of the `system.error.<name>` event a strict rejection reports.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RejectionReport {
    /// The name the rejected message claimed as its source.
    pub primitive: String,
    /// `publish` or `subscribe`.
    pub operation: String,
    /// The message type that was refused.
    pub message_type: String,
    /// The sentence explaining the refusal, as logged and as replied.
    pub reason: String,
    /// The mode in force, always `strict` for a rejection.
    pub mode: String,
}

impl RejectionReport {
    /// Describe one rejected operation.
    #[must_use]
    pub fn new(
        verdict: Verdict,
        name: &str,
        operation: Operation,
        message_type: &str,
        mode: EnforcementMode,
    ) -> Self {
        Self {
            primitive: name.to_owned(),
            operation: operation.as_str().to_owned(),
            message_type: message_type.to_owned(),
            reason: violation_reason(verdict, name, operation, message_type),
            mode: mode.as_str().to_owned(),
        }
    }
}

/// Every configured primitive's declarations, plus the mode to apply.
///
/// Built once at startup from the configuration, then shared and read per
/// message. Lookup is one hash lookup for the primitive and one for the topic.
#[derive(Debug, Clone, Default)]
pub struct DeclarationTable {
    mode: EnforcementMode,
    primitives: HashMap<String, Declarations>,
}

impl DeclarationTable {
    /// Build a table from a mode and each primitive's kind and declared lists.
    #[must_use]
    pub fn new(
        mode: EnforcementMode,
        primitives: impl IntoIterator<Item = (String, Declarations)>,
    ) -> Self {
        Self {
            mode,
            primitives: primitives.into_iter().collect(),
        }
    }

    /// The configured mode.
    #[must_use]
    pub const fn mode(&self) -> EnforcementMode {
        self.mode
    }

    /// How many primitives the table covers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.primitives.len()
    }

    /// Whether the table covers no primitives.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.primitives.is_empty()
    }

    /// Decide an operation, returning both the verdict and what to do about it.
    ///
    /// In [`EnforcementMode::Off`] no lookup happens at all, so the cost of the
    /// default is one comparison per message.
    #[must_use]
    pub fn check(&self, name: &str, operation: Operation, message_type: &str) -> Checked {
        if !self.mode.is_enforcing() {
            return Checked {
                verdict: Verdict::Declared,
                enforcement: Enforcement::Accept,
            };
        }
        let verdict = decide(self.primitives.get(name), operation, message_type);
        Checked {
            verdict,
            enforcement: enforcement_for(self.mode, verdict),
        }
    }
}

/// A verdict together with what the configured mode does about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checked {
    /// Why the operation was or was not within the declarations.
    pub verdict: Verdict,
    /// What the engine does about it.
    pub enforcement: Enforcement,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topics(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn source(publishes: &[&str]) -> Declarations {
        Declarations::new(PrimitiveKind::Source, &topics(publishes), &[])
    }

    fn handler(publishes: &[&str], subscribes: &[&str]) -> Declarations {
        Declarations::new(
            PrimitiveKind::Handler,
            &topics(publishes),
            &topics(subscribes),
        )
    }

    fn sink(subscribes: &[&str]) -> Declarations {
        Declarations::new(PrimitiveKind::Sink, &[], &topics(subscribes))
    }

    #[test]
    fn publish_verdicts_cover_every_shape_of_declaration() {
        let timer = source(&["timer.tick"]);
        let filter = handler(&["timer.filtered", "filter.processed"], &["timer.tick"]);
        let console = sink(&["timer.filtered"]);
        let wildcard = source(&["metrics.*"]);
        let everything = source(&["*"]);

        // (declarations, message type, expected verdict)
        let cases: &[(Option<&Declarations>, &str, Verdict)] = &[
            // A declared type is allowed.
            (Some(&timer), "timer.tick", Verdict::Declared),
            // An undeclared type from a real primitive is the typo case.
            (Some(&timer), "timer.tock", Verdict::Undeclared),
            // Every entry of a multi-topic declaration counts.
            (Some(&filter), "timer.filtered", Verdict::Declared),
            (Some(&filter), "filter.processed", Verdict::Declared),
            (Some(&filter), "timer.tick", Verdict::Undeclared),
            // A sink has no publishes at all, so its kind settles it.
            (Some(&console), "timer.filtered", Verdict::KindCannot),
            (Some(&console), "anything", Verdict::KindCannot),
            // A terminal wildcard permits its prefix and nothing else.
            (Some(&wildcard), "metrics.cpu", Verdict::Declared),
            (Some(&wildcard), "metrics.", Verdict::Declared),
            (Some(&wildcard), "metric.cpu", Verdict::Undeclared),
            // A bare `*` permits everything.
            (Some(&everything), "anything.at.all", Verdict::Declared),
            // A name the engine does not know.
            (None, "timer.tick", Verdict::UnknownPrimitive),
            // Protocol beats every other rule, including an unknown name.
            (None, "system.request.topology", Verdict::Protocol),
            (
                Some(&console),
                "system.request.subscriptions",
                Verdict::Protocol,
            ),
            (Some(&timer), "system.request.topology", Verdict::Protocol),
            // A system type that is not protocol is still judged normally.
            (Some(&timer), "system.error.timer", Verdict::Undeclared),
        ];

        for (declarations, message_type, expected) in cases {
            assert_eq!(
                decide(*declarations, Operation::Publish, message_type),
                *expected,
                "publish of {message_type}"
            );
        }
    }

    #[test]
    fn subscribe_verdicts_mirror_publish_against_the_other_list() {
        let timer = source(&["timer.tick"]);
        let filter = handler(&["timer.filtered"], &["timer.tick"]);
        let console = sink(&["timer.filtered", "system.error.*"]);

        let cases: &[(Option<&Declarations>, &str, Verdict)] = &[
            // A source declares no subscribes and cannot subscribe.
            (Some(&timer), "timer.tick", Verdict::KindCannot),
            (Some(&filter), "timer.tick", Verdict::Declared),
            (Some(&filter), "timer.filtered", Verdict::Undeclared),
            (Some(&console), "timer.filtered", Verdict::Declared),
            (Some(&console), "system.error.filter", Verdict::Declared),
            (Some(&console), "system.started.filter", Verdict::Undeclared),
            (None, "timer.tick", Verdict::UnknownPrimitive),
        ];

        for (declarations, message_type, expected) in cases {
            assert_eq!(
                decide(*declarations, Operation::Subscribe, message_type),
                *expected,
                "subscribe to {message_type}"
            );
        }
    }

    #[test]
    fn a_misplaced_wildcard_declaration_permits_nothing() {
        // Config validation rejects these, but a declaration that reached the
        // table anyway must not become a permit-all.
        let broken = source(&["metrics.*.cpu"]);
        assert_eq!(
            decide(Some(&broken), Operation::Publish, "metrics.host.cpu"),
            Verdict::Undeclared
        );
        assert_eq!(
            decide(Some(&broken), Operation::Publish, "metrics.*.cpu"),
            Verdict::Undeclared
        );
    }

    #[test]
    fn mode_decides_what_a_verdict_costs() {
        let cases: &[(EnforcementMode, Verdict, Enforcement)] = &[
            (
                EnforcementMode::Off,
                Verdict::Undeclared,
                Enforcement::Accept,
            ),
            (EnforcementMode::Off, Verdict::Declared, Enforcement::Accept),
            (
                EnforcementMode::Warn,
                Verdict::Declared,
                Enforcement::Accept,
            ),
            (
                EnforcementMode::Warn,
                Verdict::Protocol,
                Enforcement::Accept,
            ),
            (
                EnforcementMode::Warn,
                Verdict::Undeclared,
                Enforcement::Warn,
            ),
            (
                EnforcementMode::Warn,
                Verdict::KindCannot,
                Enforcement::Warn,
            ),
            (
                EnforcementMode::Warn,
                Verdict::UnknownPrimitive,
                Enforcement::Warn,
            ),
            (
                EnforcementMode::Strict,
                Verdict::Declared,
                Enforcement::Accept,
            ),
            (
                EnforcementMode::Strict,
                Verdict::Protocol,
                Enforcement::Accept,
            ),
            (
                EnforcementMode::Strict,
                Verdict::Undeclared,
                Enforcement::Reject,
            ),
            (
                EnforcementMode::Strict,
                Verdict::KindCannot,
                Enforcement::Reject,
            ),
            (
                EnforcementMode::Strict,
                Verdict::UnknownPrimitive,
                Enforcement::Reject,
            ),
        ];

        for (mode, verdict, expected) in cases {
            assert_eq!(
                enforcement_for(*mode, *verdict),
                *expected,
                "{mode} mode with {verdict:?}"
            );
        }
    }

    #[test]
    fn an_off_table_never_looks_anything_up() {
        let table = DeclarationTable::new(
            EnforcementMode::Off,
            [("timer".to_string(), source(&["timer.tick"]))],
        );
        let checked = table.check("nobody", Operation::Publish, "made.up");
        assert_eq!(checked.enforcement, Enforcement::Accept);
        assert!(!checked.verdict.is_violation());
    }

    #[test]
    fn a_strict_table_rejects_only_the_undeclared() {
        let table = DeclarationTable::new(
            EnforcementMode::Strict,
            [
                ("timer".to_string(), source(&["timer.tick"])),
                ("console".to_string(), sink(&["timer.tick"])),
            ],
        );
        assert_eq!(table.len(), 2);
        assert!(!table.is_empty());

        assert_eq!(
            table
                .check("timer", Operation::Publish, "timer.tick")
                .enforcement,
            Enforcement::Accept
        );
        assert_eq!(
            table
                .check("timer", Operation::Publish, "timer.tock")
                .enforcement,
            Enforcement::Reject
        );
        assert_eq!(
            table
                .check("console", Operation::Publish, "timer.tick")
                .enforcement,
            Enforcement::Reject
        );
        assert_eq!(
            table
                .check(
                    "console",
                    Operation::Publish,
                    "system.request.subscriptions"
                )
                .enforcement,
            Enforcement::Accept
        );
        assert_eq!(
            table
                .check("ghost", Operation::Publish, "timer.tick")
                .enforcement,
            Enforcement::Reject
        );
    }

    #[test]
    fn every_protocol_topic_is_allowed_to_an_unknown_client() {
        for topic in PROTOCOL_TOPICS {
            assert!(is_protocol_topic(topic), "{topic}");
            assert_eq!(
                decide(None, Operation::Publish, topic),
                Verdict::Protocol,
                "{topic}"
            );
        }
        assert!(!is_protocol_topic("system.response.topology"));
        assert!(!is_protocol_topic("system.request.somethingelse"));
    }

    #[test]
    fn reason_text_names_the_primitive_the_operation_and_the_type() {
        let reason = violation_reason(
            Verdict::Undeclared,
            "filter",
            Operation::Publish,
            "timer.tock",
        );
        assert!(reason.contains("filter"), "{reason}");
        assert!(reason.contains("publish"), "{reason}");
        assert!(reason.contains("timer.tock"), "{reason}");
        assert!(reason.contains("publishes"), "{reason}");

        let reason = violation_reason(
            Verdict::Undeclared,
            "console",
            Operation::Subscribe,
            "timer.tock",
        );
        assert!(reason.contains("subscribes"), "{reason}");

        let reason = violation_reason(
            Verdict::UnknownPrimitive,
            "ghost",
            Operation::Publish,
            "a.b",
        );
        assert!(reason.contains("no primitive of that name"), "{reason}");

        let reason = violation_reason(Verdict::KindCannot, "console", Operation::Publish, "a.b");
        assert!(reason.contains("cannot publish"), "{reason}");
    }

    #[test]
    fn modes_round_trip_through_their_config_spelling() {
        for (mode, spelling) in [
            (EnforcementMode::Off, "off"),
            (EnforcementMode::Warn, "warn"),
            (EnforcementMode::Strict, "strict"),
        ] {
            assert_eq!(mode.as_str(), spelling);
            assert_eq!(mode.to_string(), spelling);
        }
        assert_eq!(EnforcementMode::default(), EnforcementMode::Off);
        assert!(!EnforcementMode::Off.is_enforcing());
        assert!(EnforcementMode::Warn.is_enforcing());
        assert!(EnforcementMode::Strict.is_enforcing());
    }

    #[test]
    fn a_rejection_reports_under_the_primitives_own_error_topic() {
        assert_eq!(rejection_event_type("filter"), "system.error.filter");

        let report = RejectionReport::new(
            Verdict::Undeclared,
            "filter",
            Operation::Publish,
            "timer.tock",
            EnforcementMode::Strict,
        );
        assert_eq!(report.primitive, "filter");
        assert_eq!(report.operation, "publish");
        assert_eq!(report.message_type, "timer.tock");
        assert_eq!(report.mode, "strict");
        assert_eq!(
            report.reason,
            violation_reason(
                Verdict::Undeclared,
                "filter",
                Operation::Publish,
                "timer.tock"
            )
        );
    }

    #[test]
    fn an_empty_declaration_list_permits_nothing() {
        let set = TopicSet::new(&[]);
        assert!(set.is_empty());
        assert!(!set.permits("anything"));
    }
}
