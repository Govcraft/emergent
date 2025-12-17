//! Standard message envelope for all Emergent communications.
//!
//! All messages in Emergent use a standard envelope format. Developers don't create
//! new message types - they specify a `message_type` string and put their domain
//! data in the `payload` field.

use mti::prelude::*;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Standard message envelope for all Emergent communications.
///
/// # Example
///
/// ```rust
/// use emergent_engine::EmergentMessage;
/// use serde_json::json;
///
/// let message = EmergentMessage::new("timer.tick")
///     .with_source("timer_source")
///     .with_payload(json!({
///         "sequence": 42,
///         "interval_ms": 5000,
///     }));
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmergentMessage {
    /// Unique message ID (MTI format: msg_<uuid_v7>).
    pub id: String,

    /// Message type for routing (e.g., "email.received", "timer.tick").
    pub message_type: String,

    /// Source client that published this message.
    pub source: String,

    /// Optional correlation ID for request-response or tracing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,

    /// Optional causation ID (ID of message that triggered this one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<String>,

    /// Timestamp when message was created (Unix ms).
    pub timestamp_ms: u64,

    /// User-defined payload (any serializable data).
    pub payload: serde_json::Value,

    /// Optional metadata for debugging, tracing, etc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl EmergentMessage {
    /// Create a new message with the given type.
    ///
    /// The message will have a unique ID generated using MTI (UUIDv7 format)
    /// and a timestamp set to the current time.
    #[must_use]
    pub fn new(message_type: &str) -> Self {
        Self {
            id: "msg".create_type_id::<V7>().to_string(),
            message_type: message_type.to_string(),
            source: String::new(),
            correlation_id: None,
            causation_id: None,
            timestamp_ms: current_timestamp_ms(),
            payload: serde_json::Value::Null,
            metadata: None,
        }
    }

    /// Set the source of this message.
    #[must_use]
    pub fn with_source(mut self, source: &str) -> Self {
        self.source = source.to_string();
        self
    }

    /// Set the payload of this message.
    #[must_use]
    pub fn with_payload(mut self, payload: impl Serialize) -> Self {
        self.payload = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        self
    }

    /// Set the correlation ID for request-response tracking.
    #[must_use]
    pub fn with_correlation_id(mut self, id: &str) -> Self {
        self.correlation_id = Some(id.to_string());
        self
    }

    /// Set the causation ID (ID of the message that triggered this one).
    #[must_use]
    pub fn with_causation_id(mut self, id: &str) -> Self {
        self.causation_id = Some(id.to_string());
        self
    }

    /// Set metadata for this message.
    #[must_use]
    pub fn with_metadata(mut self, metadata: impl Serialize) -> Self {
        self.metadata = Some(serde_json::to_value(metadata).unwrap_or(serde_json::Value::Null));
        self
    }

    /// Get the message ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the message type.
    #[must_use]
    pub fn message_type(&self) -> &str {
        &self.message_type
    }

    /// Get the source.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Extract the payload as a typed value.
    ///
    /// # Errors
    ///
    /// Returns an error if the payload cannot be deserialized into the requested type.
    pub fn payload_as<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.payload.clone())
    }

    /// Serialize the message to MessagePack bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_msgpack(&self) -> Result<Vec<u8>, rmp_serde::encode::Error> {
        rmp_serde::to_vec(self)
    }

    /// Deserialize a message from MessagePack bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if deserialization fails.
    pub fn from_msgpack(bytes: &[u8]) -> Result<Self, rmp_serde::decode::Error> {
        rmp_serde::from_slice(bytes)
    }

    /// Serialize the message to JSON bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialize a message from JSON bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if deserialization fails.
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// Get the current timestamp in milliseconds since Unix epoch.
fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_message_creation() {
        let msg = EmergentMessage::new("timer.tick")
            .with_source("timer_source")
            .with_payload(json!({"sequence": 42}));

        assert!(msg.id.starts_with("msg_"));
        assert_eq!(msg.message_type, "timer.tick");
        assert_eq!(msg.source, "timer_source");
        assert_eq!(msg.payload["sequence"], 42);
    }

    #[test]
    fn test_causation_chain() {
        let parent = EmergentMessage::new("parent.event");
        let child = EmergentMessage::new("child.event").with_causation_id(parent.id());

        assert_eq!(child.causation_id, Some(parent.id.clone()));
    }

    #[test]
    fn test_msgpack_roundtrip() {
        let original = EmergentMessage::new("test.message")
            .with_source("test")
            .with_payload(json!({"key": "value"}));

        let bytes = original.to_msgpack().unwrap();
        let restored = EmergentMessage::from_msgpack(&bytes).unwrap();

        assert_eq!(original.id, restored.id);
        assert_eq!(original.message_type, restored.message_type);
        assert_eq!(original.payload, restored.payload);
    }

    #[test]
    fn test_json_roundtrip() {
        let original = EmergentMessage::new("test.message")
            .with_source("test")
            .with_payload(json!({"key": "value"}));

        let bytes = original.to_json().unwrap();
        let restored = EmergentMessage::from_json(&bytes).unwrap();

        assert_eq!(original.id, restored.id);
        assert_eq!(original.message_type, restored.message_type);
        assert_eq!(original.payload, restored.payload);
    }
}
