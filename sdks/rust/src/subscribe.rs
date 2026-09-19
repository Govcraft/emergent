//! Subscription types and traits for flexible topic specification.
//!
//! # Wildcards
//!
//! A subscription topic is either a literal message type or a prefix selector
//! ending in a single `*`. `tick.*` selects every message type that starts with
//! `tick.`, and `*` alone selects every message type the engine publishes,
//! including types that appear later in the run. The wildcard is terminal: a
//! `*` anywhere but the last position could never match, so it is reported as
//! an error rather than accepted and ignored.
//!
//! These are the semantics of acton-reactive 9.3's IPC prefix subscriptions,
//! which is the table the engine routes through. Engine 0.10.10 and earlier
//! accepted a wildcard topic and delivered nothing.

// ============================================================================
// Wildcard topics
// ============================================================================

/// Maximum length, in UTF-8 bytes, of a wildcard subscription topic.
///
/// The limit is the engine's, enforced by acton-reactive's IPC subscription
/// manager. It is checked here so the caller learns which topic is too long.
pub const MAX_PATTERN_LEN: usize = 256;

/// How the engine will match one requested subscription topic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TopicKind {
    /// A literal message type, delivered only on an exact string match.
    Exact,
    /// A prefix selector ending in a single terminal `*`.
    Pattern,
}

/// A subscription topic the engine could never deliver a message for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidTopic {
    /// The topic was the empty string.
    Empty,
    /// A `*` appeared anywhere but as the final character.
    MisplacedWildcard {
        /// The topic as the caller wrote it.
        topic: String,
    },
    /// A wildcard topic exceeded [`MAX_PATTERN_LEN`] bytes.
    PatternTooLong {
        /// The topic as the caller wrote it.
        topic: String,
        /// Its length in UTF-8 bytes.
        length: usize,
    },
}

impl std::fmt::Display for InvalidTopic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("subscription topic cannot be empty"),
            Self::MisplacedWildcard { topic } => write!(
                f,
                "subscription topic '{topic}' can never match: '*' is only a wildcard as the \
                 final character, so write a prefix selector such as '{prefix}*' instead",
                prefix = topic.split('*').next().unwrap_or_default()
            ),
            Self::PatternTooLong { topic, length } => write!(
                f,
                "subscription topic '{topic}' is {length} bytes, over the {MAX_PATTERN_LEN}-byte \
                 limit for a wildcard topic"
            ),
        }
    }
}

impl std::error::Error for InvalidTopic {}

/// Returns the literal prefix of a terminal-wildcard topic.
///
/// `tick.*` yields `Some("tick.")` and `*` yields `Some("")`. A topic with no
/// trailing `*` yields `None`, and so does one whose prefix contains a `*`,
/// because that topic is not a usable selector.
#[must_use]
pub fn pattern_prefix(topic: &str) -> Option<&str> {
    let prefix = topic.strip_suffix('*')?;
    if prefix.contains('*') {
        return None;
    }
    Some(prefix)
}

/// Decides whether a topic is a literal message type or a wildcard selector.
///
/// # Errors
///
/// Returns [`InvalidTopic`] when the topic is empty, when a `*` appears
/// anywhere but the final position, or when a wildcard topic is longer than
/// [`MAX_PATTERN_LEN`] bytes. Each of those can never deliver a message, so
/// the caller is told rather than left waiting.
pub fn classify_topic(topic: &str) -> std::result::Result<TopicKind, InvalidTopic> {
    if topic.is_empty() {
        return Err(InvalidTopic::Empty);
    }
    if !topic.contains('*') {
        return Ok(TopicKind::Exact);
    }
    if pattern_prefix(topic).is_none() {
        return Err(InvalidTopic::MisplacedWildcard {
            topic: topic.to_owned(),
        });
    }
    if topic.len() > MAX_PATTERN_LEN {
        return Err(InvalidTopic::PatternTooLong {
            topic: topic.to_owned(),
            length: topic.len(),
        });
    }
    Ok(TopicKind::Pattern)
}

/// Tests a message type against one subscription topic.
///
/// A literal topic matches only itself. A terminal-wildcard topic matches every
/// message type that starts with the text before the `*`, so `*` matches all of
/// them. A topic with a misplaced wildcard matches nothing.
#[must_use]
pub fn topic_matches(topic: &str, message_type: &str) -> bool {
    pattern_prefix(topic).map_or_else(
        || !topic.contains('*') && topic == message_type,
        |prefix| message_type.starts_with(prefix),
    )
}

/// Requested topics split into the two subscription kinds the engine accepts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TopicPartition {
    /// Literal message types, subscribed by name.
    pub(crate) exact: Vec<String>,
    /// Terminal-wildcard selectors, subscribed as IPC patterns.
    pub(crate) patterns: Vec<String>,
}

/// Splits requested topics into literal names and wildcard selectors.
///
/// Order within each group is the caller's order, and duplicates are kept: the
/// engine deduplicates recipients, so a connection subscribed to both
/// `tick.out` and `tick.*` still receives one copy of `tick.out`.
///
/// # Errors
///
/// Returns the first [`InvalidTopic`] found, before any subscription request is
/// sent, so a topology with an unusable topic fails loudly at startup.
pub(crate) fn partition_topics(
    topics: Vec<String>,
) -> std::result::Result<TopicPartition, InvalidTopic> {
    let mut partition = TopicPartition::default();
    for topic in topics {
        match classify_topic(&topic)? {
            TopicKind::Exact => partition.exact.push(topic),
            TopicKind::Pattern => partition.patterns.push(topic),
        }
    }
    Ok(partition)
}

/// Trait for types that can be converted into subscription topics.
///
/// This allows `subscribe()` to accept various input types:
///
/// ```rust,ignore
/// // Single topic
/// sink.subscribe("timer.tick").await?;
///
/// // Array of topics
/// sink.subscribe(["timer.tick", "timer.filtered"]).await?;
///
/// // Slice of topics
/// sink.subscribe(&["timer.tick", "timer.filtered"]).await?;
///
/// // Vec of topics
/// sink.subscribe(vec!["timer.tick".to_string()]).await?;
/// ```
pub trait IntoSubscription {
    /// Convert into a vector of topic strings.
    fn into_topics(self) -> Vec<String>;
}

impl IntoSubscription for &str {
    fn into_topics(self) -> Vec<String> {
        vec![self.to_string()]
    }
}

impl IntoSubscription for String {
    fn into_topics(self) -> Vec<String> {
        vec![self]
    }
}

impl IntoSubscription for &String {
    fn into_topics(self) -> Vec<String> {
        vec![self.clone()]
    }
}

impl<const N: usize> IntoSubscription for [&str; N] {
    fn into_topics(self) -> Vec<String> {
        self.into_iter().map(String::from).collect()
    }
}

impl<const N: usize> IntoSubscription for &[&str; N] {
    fn into_topics(self) -> Vec<String> {
        self.iter().map(|s| (*s).to_string()).collect()
    }
}

impl<const N: usize> IntoSubscription for [String; N] {
    fn into_topics(self) -> Vec<String> {
        self.into_iter().collect()
    }
}

impl IntoSubscription for &[&str] {
    fn into_topics(self) -> Vec<String> {
        self.iter().map(|s| (*s).to_string()).collect()
    }
}

impl IntoSubscription for &[String] {
    fn into_topics(self) -> Vec<String> {
        self.to_vec()
    }
}

impl IntoSubscription for Vec<String> {
    fn into_topics(self) -> Vec<String> {
        self
    }
}

impl IntoSubscription for &Vec<String> {
    fn into_topics(self) -> Vec<String> {
        self.clone()
    }
}

impl IntoSubscription for Vec<&str> {
    fn into_topics(self) -> Vec<String> {
        self.into_iter().map(String::from).collect()
    }
}

impl IntoSubscription for &Vec<&str> {
    fn into_topics(self) -> Vec<String> {
        self.iter().map(|s| (*s).to_string()).collect()
    }
}

/// Decide whether the engine must be asked for the configured subscription list.
///
/// The convenience constructors (`EmergentHandler::messages`,
/// `EmergentSink::messages`) only fall back to the engine's configuration when
/// the caller requested no topics of their own.
#[must_use]
pub(crate) fn needs_configured_topics(requested: &[String]) -> bool {
    requested.is_empty()
}

/// Resolve the topics to subscribe to from what the caller asked for and what
/// the engine has configured.
///
/// Explicitly requested topics always win. The configured list is the fallback
/// for callers that pass nothing, which keeps the engine's TOML the source of
/// truth for primitives that do not hard-code their own subscriptions.
#[must_use]
pub(crate) fn resolve_topics(requested: Vec<String>, configured: Vec<String>) -> Vec<String> {
    if requested.is_empty() {
        configured
    } else {
        requested
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topics(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_topic_without_a_star_is_exact() {
        assert_eq!(classify_topic("tick.out"), Ok(TopicKind::Exact));
        assert_eq!(classify_topic("system.shutdown"), Ok(TopicKind::Exact));
    }

    #[test]
    fn a_terminal_star_is_a_pattern() {
        assert_eq!(classify_topic("tick.*"), Ok(TopicKind::Pattern));
        assert_eq!(classify_topic("system.started.*"), Ok(TopicKind::Pattern));
        assert_eq!(classify_topic("*"), Ok(TopicKind::Pattern));
        assert_eq!(classify_topic("tick*"), Ok(TopicKind::Pattern));
    }

    #[test]
    fn a_star_anywhere_else_is_rejected() {
        assert_eq!(
            classify_topic("*.out"),
            Err(InvalidTopic::MisplacedWildcard {
                topic: "*.out".to_string()
            })
        );
        assert_eq!(
            classify_topic("tick.*.out"),
            Err(InvalidTopic::MisplacedWildcard {
                topic: "tick.*.out".to_string()
            })
        );
        assert_eq!(
            classify_topic("tick.**"),
            Err(InvalidTopic::MisplacedWildcard {
                topic: "tick.**".to_string()
            })
        );
    }

    #[test]
    fn an_empty_topic_is_rejected() {
        assert_eq!(classify_topic(""), Err(InvalidTopic::Empty));
    }

    #[test]
    fn an_oversized_pattern_is_rejected() {
        let topic = format!("{}*", "a".repeat(MAX_PATTERN_LEN));
        assert_eq!(
            classify_topic(&topic),
            Err(InvalidTopic::PatternTooLong {
                length: MAX_PATTERN_LEN + 1,
                topic,
            })
        );
    }

    #[test]
    fn a_pattern_exactly_at_the_limit_is_accepted() {
        let topic = format!("{}*", "a".repeat(MAX_PATTERN_LEN - 1));
        assert_eq!(classify_topic(&topic), Ok(TopicKind::Pattern));
    }

    #[test]
    fn the_rejection_message_names_the_topic_and_a_usable_selector() {
        let message = InvalidTopic::MisplacedWildcard {
            topic: "system.*.error".to_string(),
        }
        .to_string();
        assert!(message.contains("system.*.error"), "got: {message}");
        assert!(message.contains("'system.*'"), "got: {message}");
    }

    #[test]
    fn pattern_prefix_strips_only_a_terminal_star() {
        assert_eq!(pattern_prefix("tick.*"), Some("tick."));
        assert_eq!(pattern_prefix("*"), Some(""));
        assert_eq!(pattern_prefix("tick.out"), None);
        assert_eq!(pattern_prefix("*.out"), None);
        assert_eq!(pattern_prefix("a*b*"), None);
    }

    #[test]
    fn an_exact_topic_matches_only_itself() {
        assert!(topic_matches("tick.out", "tick.out"));
        assert!(!topic_matches("tick.out", "tick.exit"));
        assert!(!topic_matches("tick.out", "tick.out.deep"));
    }

    #[test]
    fn a_pattern_matches_every_message_type_with_its_prefix() {
        assert!(topic_matches("tick.*", "tick.out"));
        assert!(topic_matches("tick.*", "tick.exit"));
        assert!(topic_matches("tick.*", "tick.out.deep"));
        assert!(!topic_matches("tick.*", "tick"));
        assert!(!topic_matches("tick.*", "ticker.out"));
        assert!(topic_matches("system.started.*", "system.started.ticker"));
        assert!(!topic_matches("system.started.*", "system.stopped.ticker"));
    }

    #[test]
    fn a_bare_star_matches_everything() {
        assert!(topic_matches("*", "tick.out"));
        assert!(topic_matches("*", "system.shutdown"));
        assert!(topic_matches("*", ""));
    }

    #[test]
    fn a_misplaced_wildcard_matches_nothing() {
        assert!(!topic_matches("*.out", "tick.out"));
        assert!(!topic_matches("*.out", "*.out"));
    }

    #[test]
    fn partitioning_keeps_order_and_separates_the_two_kinds() -> Result<(), InvalidTopic> {
        let partition = partition_topics(topics(&["tick.out", "tick.*", "system.started.*", "a"]))?;
        assert_eq!(partition.exact, topics(&["tick.out", "a"]));
        assert_eq!(partition.patterns, topics(&["tick.*", "system.started.*"]));
        Ok(())
    }

    #[test]
    fn partitioning_reports_the_first_unusable_topic() {
        assert_eq!(
            partition_topics(topics(&["tick.out", "a*b", "c*d"])),
            Err(InvalidTopic::MisplacedWildcard {
                topic: "a*b".to_string()
            })
        );
    }

    #[test]
    fn partitioning_keeps_an_overlapping_pair_so_the_engine_can_deduplicate()
    -> Result<(), InvalidTopic> {
        let partition = partition_topics(topics(&["tick.out", "tick.*"]))?;
        assert_eq!(partition.exact, topics(&["tick.out"]));
        assert_eq!(partition.patterns, topics(&["tick.*"]));
        Ok(())
    }

    #[test]
    fn requested_topics_win_over_configured() {
        let resolved = resolve_topics(topics(&["a.b"]), topics(&["c.d", "e.f"]));
        assert_eq!(resolved, topics(&["a.b"]));
    }

    #[test]
    fn empty_request_falls_back_to_configured() {
        let resolved = resolve_topics(Vec::new(), topics(&["c.d", "e.f"]));
        assert_eq!(resolved, topics(&["c.d", "e.f"]));
    }

    #[test]
    fn empty_request_and_empty_config_resolve_to_nothing() {
        assert!(resolve_topics(Vec::new(), Vec::new()).is_empty());
    }

    #[test]
    fn requested_topics_survive_an_empty_config() {
        let resolved = resolve_topics(topics(&["a.b", "a.c"]), Vec::new());
        assert_eq!(resolved, topics(&["a.b", "a.c"]));
    }

    #[test]
    fn requested_order_and_duplicates_are_preserved() {
        let resolved = resolve_topics(topics(&["b", "a", "b"]), topics(&["z"]));
        assert_eq!(resolved, topics(&["b", "a", "b"]));
    }

    #[test]
    fn configured_list_is_only_needed_when_nothing_was_requested() {
        assert!(needs_configured_topics(&[]));
        assert!(!needs_configured_topics(&topics(&["a.b"])));
    }

    #[test]
    fn test_str_into_subscription() {
        let topics = "timer.tick".into_topics();
        assert_eq!(topics, vec!["timer.tick"]);
    }

    #[test]
    fn test_string_into_subscription() {
        let topics = String::from("timer.tick").into_topics();
        assert_eq!(topics, vec!["timer.tick"]);
    }

    #[test]
    fn test_array_into_subscription() {
        let topics = ["timer.tick", "timer.filtered"].into_topics();
        assert_eq!(topics, vec!["timer.tick", "timer.filtered"]);
    }

    #[test]
    fn test_slice_into_subscription() {
        let arr = ["timer.tick", "timer.filtered"];
        let topics = arr.as_slice().into_topics();
        assert_eq!(topics, vec!["timer.tick", "timer.filtered"]);
    }

    #[test]
    fn test_vec_into_subscription() {
        let topics = vec!["timer.tick".to_string(), "timer.filtered".to_string()].into_topics();
        assert_eq!(topics, vec!["timer.tick", "timer.filtered"]);
    }
}
