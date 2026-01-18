//! Standard message envelope for all Emergent communications.
//!
//! This module re-exports the `EmergentMessage` type from the client library
//! for use within the engine. All messages in Emergent use the standard envelope format.

// Re-export the message type from the client library
pub use emergent_client::EmergentMessage;

// Re-export the types for convenience
pub use emergent_client::types::{
    CausationId, CorrelationId, MessageId, MessageType, PrimitiveName, Timestamp,
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_message_creation() {
        let msg = EmergentMessage::new("timer.tick")
            .with_source("timer_source")
            .with_payload(json!({"sequence": 42}));

        assert!(msg.id.to_string().starts_with("msg_"));
        assert_eq!(msg.message_type.as_str(), "timer.tick");
        assert_eq!(msg.source.as_str(), "timer_source");
        assert_eq!(msg.payload["sequence"], 42);
    }

    #[test]
    fn test_new_with_id_and_timestamp_is_pure() {
        let id = MessageId::new();
        let timestamp = Timestamp::from_millis(1704067200000); // 2024-01-01 00:00:00 UTC

        let msg1 =
            EmergentMessage::new_with_id_and_timestamp("test.event", id.clone(), timestamp);
        let msg2 =
            EmergentMessage::new_with_id_and_timestamp("test.event", id.clone(), timestamp);

        // Pure function should produce identical results for identical inputs
        assert_eq!(msg1.id, msg2.id);
        assert_eq!(msg1.message_type, msg2.message_type);
        assert_eq!(msg1.timestamp_ms, msg2.timestamp_ms);
    }

    #[test]
    fn test_causation_chain() {
        let parent = EmergentMessage::new("parent.event");
        let child =
            EmergentMessage::new("child.event").with_causation_from_message(parent.id());

        assert_eq!(
            child.causation_id.as_ref().map(|c| c.to_string()),
            Some(parent.id.to_string())
        );
    }

    #[test]
    fn test_msgpack_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let original = EmergentMessage::new("test.message")
            .with_source("test")
            .with_payload(json!({"key": "value"}));

        let bytes = original.to_msgpack()?;
        let restored = EmergentMessage::from_msgpack(&bytes)?;

        assert_eq!(original.id, restored.id);
        assert_eq!(original.message_type, restored.message_type);
        assert_eq!(original.payload, restored.payload);
        Ok(())
    }

    #[test]
    fn test_json_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let original = EmergentMessage::new("test.message")
            .with_source("test")
            .with_payload(json!({"key": "value"}));

        let bytes = original.to_json()?;
        let restored = EmergentMessage::from_json(&bytes)?;

        assert_eq!(original.id, restored.id);
        assert_eq!(original.message_type, restored.message_type);
        assert_eq!(original.payload, restored.payload);
        Ok(())
    }
}
