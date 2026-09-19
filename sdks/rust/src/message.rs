//! Message types for the Emergent client library.

use crate::types::{
    CausationId, CorrelationId, InvalidMessageType, InvalidPrimitiveName, MessageId, MessageType,
    PrimitiveName, Timestamp,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

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
    /// Use [`Self::try_new`] whenever the message type comes from configuration
    /// or any other runtime string, because this constructor panics on an
    /// invalid type.
    ///
    /// # Panics
    ///
    /// Panics if the message type is invalid.
    #[must_use]
    pub fn new(message_type: &str) -> Self {
        Self::new_with_id_and_timestamp(message_type, MessageId::new(), Timestamp::now())
    }

    /// Create a new message with the given type, returning an error for an invalid type.
    ///
    /// This is the constructor to use when the message type is built from
    /// runtime data: it never panics, so an invalid type cannot take the
    /// process down.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMessageType`] if `message_type` is not a valid message type.
    ///
    /// # Example
    ///
    /// ```rust
    /// use emergent_client::EmergentMessage;
    ///
    /// assert!(EmergentMessage::try_new("system.started.timer").is_ok());
    /// assert!(EmergentMessage::try_new("system.started.Bad Name").is_err());
    /// ```
    pub fn try_new(message_type: &str) -> Result<Self, InvalidMessageType> {
        Self::try_new_with_id_and_timestamp(message_type, MessageId::new(), Timestamp::now())
    }

    /// Create a new message with explicit ID and timestamp.
    ///
    /// This is a pure function suitable for testing and deterministic message creation.
    /// For production use, prefer `new()` which generates a unique ID and current timestamp.
    ///
    /// # Panics
    ///
    /// Panics if the message type is invalid. Use
    /// [`Self::try_new_with_id_and_timestamp`] for runtime-built types.
    #[must_use]
    pub fn new_with_id_and_timestamp(
        message_type: &str,
        id: MessageId,
        timestamp_ms: Timestamp,
    ) -> Self {
        // Kept panicking for backwards compatibility. The fallible twin below
        // carries the actual logic.
        Self::try_new_with_id_and_timestamp(message_type, id, timestamp_ms)
            .unwrap_or_else(|e| panic!("invalid message type '{message_type}': {e}"))
    }

    /// Create a message with explicit ID and timestamp, returning an error for an invalid type.
    ///
    /// Pure and total: same inputs, same output, and no panic path.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMessageType`] if `message_type` is not a valid message type.
    pub fn try_new_with_id_and_timestamp(
        message_type: &str,
        id: MessageId,
        timestamp_ms: Timestamp,
    ) -> Result<Self, InvalidMessageType> {
        let msg_type = MessageType::new(message_type)?;

        Ok(Self {
            id,
            message_type: msg_type,
            // Placeholder source, overwritten by with_source()/with_source_name().
            source: PrimitiveName::unknown(),
            correlation_id: None,
            causation_id: None,
            timestamp_ms,
            payload: serde_json::Value::Null,
            metadata: None,
        })
    }

    /// Set the source of this message.
    ///
    /// # Panics
    ///
    /// Panics if the source name is invalid. Use [`Self::try_with_source`] for
    /// runtime-built names, or [`Self::with_source_name`] when you already hold
    /// a validated [`PrimitiveName`].
    #[must_use]
    pub fn with_source(self, source: &str) -> Self {
        self.try_with_source(source)
            .unwrap_or_else(|e| panic!("invalid source name '{source}': {e}"))
    }

    /// Set the source of this message, returning an error for an invalid name.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidPrimitiveName`] if `source` is not a valid primitive name.
    pub fn try_with_source(mut self, source: &str) -> Result<Self, InvalidPrimitiveName> {
        self.source = PrimitiveName::new(source)?;
        Ok(self)
    }

    /// Set the source of this message from an already validated name.
    ///
    /// Infallible by construction: the name has been validated already.
    #[must_use]
    pub fn with_source_name(mut self, source: PrimitiveName) -> Self {
        self.source = source;
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

    /// Set the correlation ID from an optional value.
    ///
    /// Useful for copying correlation IDs from request messages to responses.
    #[must_use]
    pub fn with_correlation_id_option(mut self, id: Option<&CorrelationId>) -> Self {
        self.correlation_id = id.cloned();
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

    /// Check whether this message has an exec-source payload shape.
    ///
    /// Returns `true` if the payload is a JSON object with a `stdout` string field,
    /// which is the envelope format produced by the `exec-source` primitive.
    #[must_use]
    pub fn has_stdout_payload(&self) -> bool {
        self.payload
            .as_object()
            .and_then(|obj| obj.get("stdout"))
            .is_some_and(serde_json::Value::is_string)
    }

    /// Unwrap an exec-source payload by extracting and parsing the `stdout` field.
    ///
    /// If the payload is an object with a `stdout` string field (the envelope format
    /// produced by `exec-source`), extracts that string and attempts to parse it as
    /// JSON. If parsing succeeds, the payload is replaced with the parsed value. If
    /// `stdout` is not valid JSON, the payload is replaced with the raw string value.
    ///
    /// If the payload does not have the exec-source shape, the message is returned
    /// unchanged.
    ///
    /// This eliminates the need for a dedicated exec-handler running
    /// `jq -c '.stdout | fromjson'` in the pipeline.
    #[must_use]
    pub fn unwrap_stdout(mut self) -> Self {
        if let Some(stdout) = self
            .payload
            .as_object()
            .and_then(|obj| obj.get("stdout"))
            .and_then(serde_json::Value::as_str)
            .map(String::from)
        {
            self.payload =
                serde_json::from_str(&stdout).unwrap_or(serde_json::Value::String(stdout));
        }
        self
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
    fn test_unwrap_stdout_json() {
        let msg = EmergentMessage::new("batch.raw").with_payload(json!({
            "command": "jq -s .",
            "stdout": "{\"transactions\":[1,2,3]}",
            "exit_code": 0
        }));

        assert!(msg.has_stdout_payload());
        let unwrapped = msg.unwrap_stdout();
        assert_eq!(unwrapped.payload(), &json!({"transactions": [1, 2, 3]}));
    }

    #[test]
    fn test_unwrap_stdout_plain_text() {
        let msg = EmergentMessage::new("exec.output").with_payload(json!({
            "command": "echo hello",
            "stdout": "hello world",
            "exit_code": 0
        }));

        let unwrapped = msg.unwrap_stdout();
        assert_eq!(unwrapped.payload(), &json!("hello world"));
    }

    #[test]
    fn test_unwrap_stdout_no_stdout_field() {
        let msg = EmergentMessage::new("timer.tick").with_payload(json!({"count": 42}));

        assert!(!msg.has_stdout_payload());
        let unwrapped = msg.unwrap_stdout();
        assert_eq!(unwrapped.payload(), &json!({"count": 42}));
    }

    #[test]
    fn test_unwrap_stdout_system_event_passthrough() {
        let msg =
            EmergentMessage::new("system.started.foo").with_payload(json!({"kind": "handler"}));

        assert!(!msg.has_stdout_payload());
        let unwrapped = msg.unwrap_stdout();
        assert_eq!(unwrapped.payload(), &json!({"kind": "handler"}));
    }

    #[test]
    fn try_new_accepts_a_valid_type() -> Result<(), Box<dyn std::error::Error>> {
        let msg = EmergentMessage::try_new("system.started.my-sink")?;
        assert_eq!(msg.message_type.as_str(), "system.started.my-sink");
        assert!(msg.source.is_default());
        Ok(())
    }

    #[test]
    fn try_new_rejects_a_type_built_from_an_invalid_name() {
        // The exact string the engine used to build from a config name with a
        // space, which aborted the process (Govcraft/emergent#42).
        assert!(matches!(
            EmergentMessage::try_new("system.started.Bad Name"),
            Err(InvalidMessageType::InvalidCharacters { .. })
        ));
    }

    #[test]
    fn try_new_with_id_and_timestamp_is_pure() {
        let id = MessageId::new();
        let timestamp = Timestamp::from_millis(1704067200000);

        let first =
            EmergentMessage::try_new_with_id_and_timestamp("test.event", id.clone(), timestamp);
        let second = EmergentMessage::try_new_with_id_and_timestamp("test.event", id, timestamp);

        assert_eq!(first.is_ok(), second.is_ok());
        if let (Ok(a), Ok(b)) = (first, second) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.message_type, b.message_type);
            assert_eq!(a.timestamp_ms, b.timestamp_ms);
        }
    }

    #[test]
    fn try_with_source_accepts_and_rejects_the_same_names_as_primitive_name() {
        let msg = EmergentMessage::new("test.event");
        assert!(msg.clone().try_with_source("emergent-engine").is_ok());
        assert!(matches!(
            msg.try_with_source("Bad Name"),
            Err(InvalidPrimitiveName::InvalidStructure { .. })
        ));
    }

    #[test]
    fn with_source_name_takes_an_already_validated_name() -> Result<(), Box<dyn std::error::Error>>
    {
        let name = PrimitiveName::new("emergent-engine")?;
        let msg = EmergentMessage::new("test.event").with_source_name(name.clone());
        assert_eq!(msg.source, name);
        Ok(())
    }

    #[test]
    #[should_panic(expected = "invalid message type 'Bad Type'")]
    fn new_still_panics_on_an_invalid_type() {
        let _ = EmergentMessage::new("Bad Type");
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
