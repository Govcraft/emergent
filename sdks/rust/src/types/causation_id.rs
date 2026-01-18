//! Causation ID type using TypeID format.

use super::MessageId;
use mti::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// A causation identifier tracking which message triggered this one.
///
/// Format: `cau_<uuid_v7>` or `msg_<uuid_v7>` (when derived from a MessageId)
/// Example: `cau_01h455vb4pex5vsknk084sn02q`
///
/// Causation IDs form a chain that can be traced back to understand
/// the sequence of events that led to a particular message.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CausationId(MagicTypeId);

/// Error returned when parsing an invalid causation ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidCausationId {
    /// TypeID parsing failed.
    Parse(MagicTypeIdError),
    /// Wrong prefix (expected "cau" or "msg").
    WrongPrefix {
        expected: &'static str,
        actual: String,
    },
}

impl fmt::Display for InvalidCausationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "invalid causation ID: {e}"),
            Self::WrongPrefix { expected, actual } => {
                write!(f, "expected prefix '{expected}', got '{actual}'")
            }
        }
    }
}

impl std::error::Error for InvalidCausationId {}

impl CausationId {
    /// The TypeID prefix for causation identifiers.
    pub const PREFIX: &'static str = "cau";

    /// Creates a new causation ID with a fresh UUIDv7 (time-sortable).
    #[must_use]
    pub fn new() -> Self {
        Self(Self::PREFIX.create_type_id::<V7>())
    }

    /// Parses a causation ID from a string.
    ///
    /// Accepts both "cau_" and "msg_" prefixes since causation IDs
    /// often reference message IDs.
    ///
    /// # Errors
    ///
    /// Returns an error if the string is not a valid TypeID or has an invalid prefix.
    pub fn parse(s: &str) -> Result<Self, InvalidCausationId> {
        let id = MagicTypeId::from_str(s).map_err(InvalidCausationId::Parse)?;

        let prefix = id.prefix().as_str();
        if prefix != Self::PREFIX && prefix != "msg" {
            return Err(InvalidCausationId::WrongPrefix {
                expected: "cau or msg",
                actual: prefix.to_string(),
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

impl Default for CausationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CausationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for CausationId {
    type Err = InvalidCausationId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl AsRef<MagicTypeId> for CausationId {
    fn as_ref(&self) -> &MagicTypeId {
        &self.0
    }
}

/// Allow converting MessageId to CausationId.
///
/// This is a common pattern where the ID of the message that triggered
/// a new message becomes the causation ID of that new message.
impl From<MessageId> for CausationId {
    fn from(msg_id: MessageId) -> Self {
        Self(msg_id.inner().clone())
    }
}

/// Allow converting a reference to MessageId to CausationId.
impl From<&MessageId> for CausationId {
    fn from(msg_id: &MessageId) -> Self {
        Self(msg_id.inner().clone())
    }
}

impl Serialize for CausationId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CausationId {
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
    fn new_creates_valid_causation_id() {
        let id = CausationId::new();
        assert!(id.to_string().starts_with("cau_"));
    }

    #[test]
    fn parse_valid_causation_id() {
        let id_str = CausationId::new().to_string();
        let parsed = CausationId::parse(&id_str);
        assert!(parsed.is_ok());
    }

    #[test]
    fn parse_accepts_msg_prefix() {
        let result = CausationId::parse("msg_01h455vb4pex5vsknk084sn02q");
        assert!(result.is_ok());
    }

    #[test]
    fn parse_wrong_prefix_fails() {
        let result = CausationId::parse("cor_01h455vb4pex5vsknk084sn02q");
        assert!(matches!(result, Err(InvalidCausationId::WrongPrefix { .. })));
    }

    #[test]
    fn causation_ids_are_unique() {
        let id1 = CausationId::new();
        let id2 = CausationId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn from_message_id() {
        let msg_id = MessageId::new();
        let msg_id_str = msg_id.to_string();
        let causation_id: CausationId = msg_id.into();
        // The causation ID preserves the msg_ prefix
        assert_eq!(causation_id.to_string(), msg_id_str);
    }

    #[test]
    fn from_message_id_ref() {
        let msg_id = MessageId::new();
        let causation_id: CausationId = (&msg_id).into();
        assert_eq!(causation_id.to_string(), msg_id.to_string());
    }

    #[test]
    fn serde_roundtrip() {
        let id = CausationId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        let restored: CausationId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, restored);
    }
}
