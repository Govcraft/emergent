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

#[cfg(test)]
mod tests {
    use super::*;

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
