//! Correlation ID type using TypeID format.

use mti::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// A correlation identifier for request-response tracking.
///
/// Format: `cor_<uuid_v7>`
/// Example: `cor_01h455vb4pex5vsknk084sn02q`
///
/// Correlation IDs link related messages together, such as requests and their responses.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CorrelationId(MagicTypeId);

/// Error returned when parsing an invalid correlation ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidCorrelationId {
    /// TypeID parsing failed.
    Parse(MagicTypeIdError),
    /// Wrong prefix (expected "cor").
    WrongPrefix {
        expected: &'static str,
        actual: String,
    },
}

impl fmt::Display for InvalidCorrelationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "invalid correlation ID: {e}"),
            Self::WrongPrefix { expected, actual } => {
                write!(f, "expected prefix '{expected}', got '{actual}'")
            }
        }
    }
}

impl std::error::Error for InvalidCorrelationId {}

impl CorrelationId {
    /// The TypeID prefix for correlation identifiers.
    pub const PREFIX: &'static str = "cor";

    /// Creates a new correlation ID with a fresh UUIDv7 (time-sortable).
    #[must_use]
    pub fn new() -> Self {
        Self(Self::PREFIX.create_type_id::<V7>())
    }

    /// Parses a correlation ID from a string, validating the prefix.
    ///
    /// # Errors
    ///
    /// Returns an error if the string is not a valid TypeID or has the wrong prefix.
    pub fn parse(s: &str) -> Result<Self, InvalidCorrelationId> {
        let id = MagicTypeId::from_str(s).map_err(InvalidCorrelationId::Parse)?;

        if id.prefix().as_str() != Self::PREFIX {
            return Err(InvalidCorrelationId::WrongPrefix {
                expected: Self::PREFIX,
                actual: id.prefix().as_str().to_string(),
            });
        }

        Ok(Self(id))
    }

    /// Returns the underlying MagicTypeId.
    #[must_use]
    pub fn inner(&self) -> &MagicTypeId {
        &self.0
    }

    /// Returns the TypeID suffix (base32-encoded UUID).
    #[must_use]
    pub fn suffix(&self) -> String {
        self.0.suffix().to_string()
    }
}

impl Default for CorrelationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CorrelationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for CorrelationId {
    type Err = InvalidCorrelationId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl AsRef<MagicTypeId> for CorrelationId {
    fn as_ref(&self) -> &MagicTypeId {
        &self.0
    }
}

impl Serialize for CorrelationId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CorrelationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_valid_correlation_id() {
        let id = CorrelationId::new();
        assert!(id.to_string().starts_with("cor_"));
    }

    #[test]
    fn parse_valid_correlation_id() {
        let id_str = CorrelationId::new().to_string();
        let parsed = CorrelationId::parse(&id_str);
        assert!(parsed.is_ok());
    }

    #[test]
    fn parse_wrong_prefix_fails() {
        let result = CorrelationId::parse("msg_01h455vb4pex5vsknk084sn02q");
        assert!(matches!(
            result,
            Err(InvalidCorrelationId::WrongPrefix { expected: "cor", .. })
        ));
    }

    #[test]
    fn correlation_ids_are_unique() {
        let id1 = CorrelationId::new();
        let id2 = CorrelationId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn display_format() {
        let id = CorrelationId::new();
        let s = id.to_string();
        assert!(s.starts_with("cor_"));
        assert_eq!(s.len(), 30); // "cor_" (4) + suffix (26)
    }

    #[test]
    fn serde_roundtrip() {
        let id = CorrelationId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        let restored: CorrelationId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, restored);
    }
}
