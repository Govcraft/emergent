//! Message type newtype for validated message type identifiers.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// A validated message type identifier.
///
/// Message types use dot-separated namespacing convention.
/// Examples: "timer.tick", "user.created", "system.shutdown"
///
/// # Validation Rules
///
/// - Cannot be empty
/// - Contain only lowercase letters, digits, dots, hyphens, underscores
/// - Cannot start or end with a dot
/// - Cannot have consecutive dots
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageType(String);

/// Error returned when creating an invalid message type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidMessageType {
    /// Message type cannot be empty.
    Empty,
    /// Message type contains invalid characters.
    InvalidCharacters { value: String },
    /// Message type has invalid structure.
    InvalidStructure { value: String, reason: &'static str },
}

impl fmt::Display for InvalidMessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "message type cannot be empty"),
            Self::InvalidCharacters { value } => {
                write!(f, "message type '{value}' contains invalid characters")
            }
            Self::InvalidStructure { value, reason } => {
                write!(f, "message type '{value}' is invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for InvalidMessageType {}

impl MessageType {
    /// Creates a new message type after validation.
    ///
    /// # Errors
    ///
    /// Returns an error if the message type is invalid.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidMessageType> {
        let value = value.into();

        if value.is_empty() {
            return Err(InvalidMessageType::Empty);
        }

        if value.starts_with('.') || value.ends_with('.') {
            return Err(InvalidMessageType::InvalidStructure {
                value,
                reason: "cannot start or end with dot",
            });
        }

        if value.contains("..") {
            return Err(InvalidMessageType::InvalidStructure {
                value,
                reason: "cannot contain consecutive dots",
            });
        }

        if !value.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-' || c == '_'
        }) {
            return Err(InvalidMessageType::InvalidCharacters { value });
        }

        Ok(Self(value))
    }

    /// Returns the message type as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the namespace parts.
    ///
    /// For "user.created", returns `["user", "created"]`.
    #[must_use]
    pub fn parts(&self) -> Vec<&str> {
        self.0.split('.').collect()
    }

    /// Returns the first part of the message type (the category).
    ///
    /// For "user.created", returns `Some("user")`.
    #[must_use]
    pub fn category(&self) -> Option<&str> {
        self.0.split('.').next()
    }

    /// Checks if this message type matches a pattern with wildcards.
    ///
    /// Supports `*` at the end to match any suffix.
    /// Examples:
    /// - "timer.tick" matches "timer.tick" (exact)
    /// - "timer.tick" matches "timer.*" (wildcard)
    /// - "system.started.timer" matches "system.started.*" (wildcard)
    #[must_use]
    pub fn matches_pattern(&self, pattern: &str) -> bool {
        if let Some(prefix) = pattern.strip_suffix(".*") {
            self.0.starts_with(prefix)
                && self.0.len() > prefix.len()
                && self.0.as_bytes().get(prefix.len()) == Some(&b'.')
        } else {
            self.0 == pattern
        }
    }
}

impl fmt::Display for MessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for MessageType {
    type Err = InvalidMessageType;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl AsRef<str> for MessageType {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Serialize for MessageType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MessageType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_message_types() {
        assert!(MessageType::new("timer.tick").is_ok());
        assert!(MessageType::new("user.created").is_ok());
        assert!(MessageType::new("system.shutdown").is_ok());
        assert!(MessageType::new("event").is_ok());
        assert!(MessageType::new("namespace.sub.event").is_ok());
        assert!(MessageType::new("with-hyphen").is_ok());
        assert!(MessageType::new("with_underscore").is_ok());
        assert!(MessageType::new("with123numbers").is_ok());
    }

    #[test]
    fn empty_is_invalid() {
        let result = MessageType::new("");
        assert!(matches!(result, Err(InvalidMessageType::Empty)));
    }

    #[test]
    fn starting_with_dot_is_invalid() {
        let result = MessageType::new(".invalid");
        assert!(matches!(
            result,
            Err(InvalidMessageType::InvalidStructure { .. })
        ));
    }

    #[test]
    fn ending_with_dot_is_invalid() {
        let result = MessageType::new("invalid.");
        assert!(matches!(
            result,
            Err(InvalidMessageType::InvalidStructure { .. })
        ));
    }

    #[test]
    fn consecutive_dots_invalid() {
        let result = MessageType::new("in..valid");
        assert!(matches!(
            result,
            Err(InvalidMessageType::InvalidStructure { .. })
        ));
    }

    #[test]
    fn uppercase_is_invalid() {
        let result = MessageType::new("Invalid");
        assert!(matches!(
            result,
            Err(InvalidMessageType::InvalidCharacters { .. })
        ));
    }

    #[test]
    fn parts_extraction() {
        let msg_type = MessageType::new("user.created").expect("valid");
        assert_eq!(msg_type.parts(), vec!["user", "created"]);
    }

    #[test]
    fn category_extraction() {
        let msg_type = MessageType::new("user.created").expect("valid");
        assert_eq!(msg_type.category(), Some("user"));
    }

    #[test]
    fn matches_pattern_exact() {
        let msg_type = MessageType::new("timer.tick").expect("valid");
        assert!(msg_type.matches_pattern("timer.tick"));
        assert!(!msg_type.matches_pattern("timer.tock"));
    }

    #[test]
    fn matches_pattern_wildcard() {
        let msg_type = MessageType::new("system.started.timer").expect("valid");
        assert!(msg_type.matches_pattern("system.started.*"));
        assert!(msg_type.matches_pattern("system.*"));
        assert!(!msg_type.matches_pattern("user.*"));
    }

    #[test]
    fn serde_roundtrip() {
        let msg_type = MessageType::new("timer.tick").expect("valid");
        let json = serde_json::to_string(&msg_type).expect("serialize");
        let restored: MessageType = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(msg_type, restored);
    }

    #[test]
    fn from_str_works() {
        let msg_type: MessageType = "timer.tick".parse().expect("parse");
        assert_eq!(msg_type.as_str(), "timer.tick");
    }
}
