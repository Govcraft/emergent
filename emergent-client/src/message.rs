//! Message types for the Emergent client library.

use mti::prelude::*;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Standard message envelope for all Emergent communications.
///
/// All messages in Emergent use this standard envelope format. Developers specify
/// a `message_type` string and put their domain data in the `payload` field.
///
/// # Example
///
/// ```rust
/// use emergent_client::EmergentMessage;
/// use serde_json::json;
///
/// let message = EmergentMessage::new("user.created")
///     .with_source("user_service")
///     .with_payload(json!({
///         "user_id": "u_12345",
///         "email": "user@example.com"
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
    /// Generates a unique ID and sets the current timestamp.
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

    /// Set the correlation ID (for request-response patterns).
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

    /// Set optional metadata.
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

    /// Get the raw payload value.
    #[must_use]
    pub fn payload(&self) -> &serde_json::Value {
        &self.payload
    }

    /// Deserialize the payload into a specific type.
    ///
    /// # Errors
    ///
    /// Returns an error if the payload cannot be deserialized into type `T`.
    pub fn payload_as<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.payload.clone())
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
    pub fn from_json(data: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(data)
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
    pub fn from_msgpack(data: &[u8]) -> Result<Self, rmp_serde::decode::Error> {
        rmp_serde::from_slice(data)
    }
}

/// Get the current timestamp in milliseconds.
fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_message_creation() {
        let msg = EmergentMessage::new("test.event")
            .with_source("test_source")
            .with_payload(json!({"key": "value"}));

        assert!(msg.id.starts_with("msg_"));
        assert_eq!(msg.message_type, "test.event");
        assert_eq!(msg.source, "test_source");
        assert!(msg.timestamp_ms > 0);
    }

    #[test]
    fn test_message_serialization() {
        let msg = EmergentMessage::new("test.event")
            .with_source("test")
            .with_payload(json!({"num": 42}));

        // Test JSON serialization
        let json_bytes = msg.to_json().unwrap();
        let from_json = EmergentMessage::from_json(&json_bytes).unwrap();
        assert_eq!(from_json.message_type, "test.event");

        // Test MessagePack serialization
        let msgpack_bytes = msg.to_msgpack().unwrap();
        let from_msgpack = EmergentMessage::from_msgpack(&msgpack_bytes).unwrap();
        assert_eq!(from_msgpack.message_type, "test.event");
    }

    #[test]
    fn test_payload_extraction() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct TestPayload {
            count: u32,
            name: String,
        }

        let msg = EmergentMessage::new("test.event").with_payload(json!({
            "count": 42,
            "name": "test"
        }));

        let payload: TestPayload = msg.payload_as().unwrap();
        assert_eq!(payload.count, 42);
        assert_eq!(payload.name, "test");
    }

    #[test]
    fn test_message_tracing() {
        let original = EmergentMessage::new("request");
        let response = EmergentMessage::new("response")
            .with_causation_id(original.id())
            .with_correlation_id("corr_123");

        assert_eq!(response.causation_id.as_deref(), Some(original.id()));
        assert_eq!(response.correlation_id.as_deref(), Some("corr_123"));
    }
}
