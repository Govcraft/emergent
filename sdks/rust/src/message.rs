//! Message types for the Emergent client library.

use crate::types::{CausationId, CorrelationId, MessageId, MessageType, PrimitiveName, Timestamp};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

/// Create a new message with the given type.
///
/// This is a convenience factory function that matches the Python and TypeScript SDKs.
///
/// # Panics
///
/// Panics if the message type is invalid.
///
/// # Example
///
/// ```rust
/// use emergent_client::create_message;
/// use serde_json::json;
///
/// let msg = create_message("timer.tick")
///     .with_payload(json!({"count": 1}))
///     .with_metadata(json!({"trace_id": "abc123"}));
/// ```
#[must_use]
pub fn create_message(message_type: impl AsRef<str>) -> EmergentMessage {
    EmergentMessage::new(message_type.as_ref())
}

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
    /// Unique message ID (TypeID format: msg_<uuid_v7>).
    pub id: MessageId,

    /// Message type for routing (e.g., "email.received", "timer.tick").
    pub message_type: MessageType,

    /// Source client that published this message.
    pub source: PrimitiveName,

    /// Optional correlation ID for request-response or tracing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,

    /// Optional causation ID (ID of message that triggered this one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<CausationId>,

    /// Timestamp when message was created (Unix ms).
    pub timestamp_ms: Timestamp,

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
    ///
    /// # Panics
    ///
    /// Panics if the message type is invalid.
    #[must_use]
    pub fn new(message_type: &str) -> Self {
        Self::new_with_id_and_timestamp(
            message_type,
            MessageId::new(),
            Timestamp::now(),
        )
    }

    /// Create a new message with explicit ID and timestamp.
    ///
    /// This is a pure function suitable for testing and deterministic message creation.
    /// For production use, prefer `new()` which generates a unique ID and current timestamp.
    ///
    /// # Panics
    ///
    /// Panics if the message type is invalid.
    #[must_use]
    pub fn new_with_id_and_timestamp(message_type: &str, id: MessageId, timestamp_ms: Timestamp) -> Self {
        // For backwards compatibility, we'll panic on invalid message types
        // In a future version, we might want to return Result instead
        let msg_type = MessageType::new(message_type)
            .unwrap_or_else(|e| panic!("invalid message type '{message_type}': {e}"));

        // Use a default source that can be overwritten with with_source()
        // We use "unknown" as a valid placeholder
        let source = PrimitiveName::new("unknown")
            .unwrap_or_else(|e| panic!("failed to create default source: {e}"));

        Self {
            id,
            message_type: msg_type,
            source,
            correlation_id: None,
            causation_id: None,
            timestamp_ms,
            payload: serde_json::Value::Null,
            metadata: None,
        }
    }

    /// Set the source of this message.
    ///
    /// # Panics
    ///
    /// Panics if the source name is invalid.
    #[must_use]
    pub fn with_source(mut self, source: &str) -> Self {
        self.source = PrimitiveName::new(source)
            .unwrap_or_else(|e| panic!("invalid source name '{source}': {e}"));
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
    pub fn with_correlation_id(mut self, id: impl Into<CorrelationId>) -> Self {
        self.correlation_id = Some(id.into());
        self
    }

    /// Set the causation ID (ID of the message that triggered this one).
    #[must_use]
    pub fn with_causation_id(mut self, id: impl Into<CausationId>) -> Self {
        self.causation_id = Some(id.into());
        self
    }

    /// Set the causation ID from a MessageId.
    ///
    /// This is a convenience method that converts the MessageId to a CausationId.
    #[must_use]
    pub fn with_causation_from_message(mut self, msg_id: &MessageId) -> Self {
        self.causation_id = Some(CausationId::from(msg_id));
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
    pub fn id(&self) -> &MessageId {
        &self.id
    }

    /// Get the message type.
    #[must_use]
    pub fn message_type(&self) -> &MessageType {
        &self.message_type
    }

    /// Get the source.
    #[must_use]
    pub fn source(&self) -> &PrimitiveName {
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
        rmp_serde::to_vec_named(self)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_message_creation() {
        let msg = EmergentMessage::new("test.event")
            .with_source("test_source")
            .with_payload(json!({"key": "value"}));

        assert!(msg.id.to_string().starts_with("msg_"));
        assert_eq!(msg.message_type.as_str(), "test.event");
        assert_eq!(msg.source.as_str(), "test_source");
        assert!(msg.timestamp_ms.as_millis() > 0);
    }

    #[test]
    fn test_message_serialization() -> Result<(), Box<dyn std::error::Error>> {
        let msg = EmergentMessage::new("test.event")
            .with_source("test")
            .with_payload(json!({"num": 42}));

        // Test JSON serialization
        let json_bytes = msg.to_json()?;
        let from_json = EmergentMessage::from_json(&json_bytes)?;
        assert_eq!(from_json.message_type.as_str(), "test.event");

        // Test MessagePack serialization
        let msgpack_bytes = msg.to_msgpack()?;
        let from_msgpack = EmergentMessage::from_msgpack(&msgpack_bytes)?;
        assert_eq!(from_msgpack.message_type.as_str(), "test.event");
        Ok(())
    }

    #[test]
    fn test_payload_extraction() -> Result<(), Box<dyn std::error::Error>> {
        #[derive(Debug, Deserialize, PartialEq)]
        struct TestPayload {
            count: u32,
            name: String,
        }

        let msg = EmergentMessage::new("test.event").with_payload(json!({
            "count": 42,
            "name": "test"
        }));

        let payload: TestPayload = msg.payload_as()?;
        assert_eq!(payload.count, 42);
        assert_eq!(payload.name, "test");
        Ok(())
    }

    #[test]
    fn test_message_tracing() {
        let original = EmergentMessage::new("request");
        let response = EmergentMessage::new("response")
            .with_causation_from_message(original.id())
            .with_correlation_id(CorrelationId::new());

        assert_eq!(
            response.causation_id.as_ref().map(|c| c.to_string()),
            Some(original.id().to_string())
        );
        assert!(response.correlation_id.is_some());
    }

    #[test]
    fn test_new_with_id_and_timestamp_is_pure() {
        let id = MessageId::new();
        let timestamp = Timestamp::from_millis(1704067200000); // 2024-01-01 00:00:00 UTC

        let msg1 = EmergentMessage::new_with_id_and_timestamp("test.event", id.clone(), timestamp);
        let msg2 = EmergentMessage::new_with_id_and_timestamp("test.event", id.clone(), timestamp);

        // Pure function should produce identical results for identical inputs
        assert_eq!(msg1.id, msg2.id);
        assert_eq!(msg1.message_type, msg2.message_type);
        assert_eq!(msg1.timestamp_ms, msg2.timestamp_ms);
    }
}
