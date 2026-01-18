//! Primitive name newtype for validated identifier strings.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// A validated primitive name identifier.
///
/// Primitive names must be valid identifiers that can be used in
/// filenames, environment variables, and configuration.
/// Examples: "timer", "user_service", "email-handler"
///
/// # Validation Rules
///
/// - Cannot be empty
/// - Must start with a lowercase letter
/// - Contain only lowercase letters, digits, hyphens, underscores
/// - Cannot exceed 64 characters
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrimitiveName(String);

/// Error returned when creating an invalid primitive name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidPrimitiveName {
    /// Primitive name cannot be empty.
    Empty,
    /// Primitive name contains invalid characters.
    InvalidCharacters { value: String },
    /// Primitive name has invalid structure.
    InvalidStructure { value: String, reason: &'static str },
}

impl fmt::Display for InvalidPrimitiveName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "primitive name cannot be empty"),
            Self::InvalidCharacters { value } => {
                write!(f, "primitive name '{value}' contains invalid characters")
            }
            Self::InvalidStructure { value, reason } => {
                write!(f, "primitive name '{value}' is invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for InvalidPrimitiveName {}

impl PrimitiveName {
    /// Maximum length for a primitive name.
    pub const MAX_LENGTH: usize = 64;

    /// Creates a new primitive name after validation.
    ///
    /// # Errors
    ///
    /// Returns an error if the primitive name is invalid.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidPrimitiveName> {
        let value = value.into();

        if value.is_empty() {
            return Err(InvalidPrimitiveName::Empty);
        }

        if value.len() > Self::MAX_LENGTH {
            return Err(InvalidPrimitiveName::InvalidStructure {
                value,
                reason: "cannot exceed 64 characters",
            });
        }

        let mut chars = value.chars();
        if let Some(first) = chars.next()
            && !first.is_ascii_lowercase()
        {
            return Err(InvalidPrimitiveName::InvalidStructure {
                value,
                reason: "must start with lowercase letter",
            });
        }

        if !value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            return Err(InvalidPrimitiveName::InvalidCharacters { value });
        }

        Ok(Self(value))
    }

    /// Returns the primitive name as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns true if this is the default "unknown" value.
    ///
    /// This is used to check if a source has been explicitly set.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.0 == "unknown"
    }
}

impl fmt::Display for PrimitiveName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for PrimitiveName {
    type Err = InvalidPrimitiveName;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl AsRef<str> for PrimitiveName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Serialize for PrimitiveName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PrimitiveName {
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
    fn valid_primitive_names() {
        assert!(PrimitiveName::new("timer").is_ok());
        assert!(PrimitiveName::new("user_service").is_ok());
        assert!(PrimitiveName::new("email-handler").is_ok());
        assert!(PrimitiveName::new("filter123").is_ok());
        assert!(PrimitiveName::new("my_handler_v2").is_ok());
    }

    #[test]
    fn empty_is_invalid() {
        let result = PrimitiveName::new("");
        assert!(matches!(result, Err(InvalidPrimitiveName::Empty)));
    }

    #[test]
    fn starting_with_digit_is_invalid() {
        let result = PrimitiveName::new("1invalid");
        assert!(matches!(
            result,
            Err(InvalidPrimitiveName::InvalidStructure { .. })
        ));
    }

    #[test]
    fn starting_with_uppercase_is_invalid() {
        let result = PrimitiveName::new("Invalid");
        assert!(matches!(
            result,
            Err(InvalidPrimitiveName::InvalidStructure { .. })
        ));
    }

    #[test]
    fn uppercase_in_middle_is_invalid() {
        let result = PrimitiveName::new("inValid");
        assert!(matches!(
            result,
            Err(InvalidPrimitiveName::InvalidCharacters { .. })
        ));
    }

    #[test]
    fn too_long_is_invalid() {
        let result = PrimitiveName::new("a".repeat(65));
        assert!(matches!(
            result,
            Err(InvalidPrimitiveName::InvalidStructure { .. })
        ));
    }

    #[test]
    fn max_length_is_valid() {
        let result = PrimitiveName::new("a".repeat(64));
        assert!(result.is_ok());
    }

    #[test]
    fn special_characters_invalid() {
        assert!(PrimitiveName::new("has space").is_err());
        assert!(PrimitiveName::new("has.dot").is_err());
        assert!(PrimitiveName::new("has@at").is_err());
    }

    #[test]
    fn serde_roundtrip() {
        let name = PrimitiveName::new("timer").expect("valid");
        let json = serde_json::to_string(&name).expect("serialize");
        let restored: PrimitiveName = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(name, restored);
    }

    #[test]
    fn from_str_works() {
        let name: PrimitiveName = "timer".parse().expect("parse");
        assert_eq!(name.as_str(), "timer");
    }
}
