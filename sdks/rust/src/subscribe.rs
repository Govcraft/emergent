//! Subscription types and traits for flexible topic specification.

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
