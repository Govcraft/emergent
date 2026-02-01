//! Typed payload helpers for system lifecycle events.
//!
//! System events are broadcast by the Emergent engine to notify primitives
//! about lifecycle changes. These helpers provide type-safe deserialization
//! of system event payloads.
//!
//! # System Event Types
//!
//! | Event Type | Payload | Description |
//! |------------|---------|-------------|
//! | `system.started.<name>` | [`SystemEventPayload`] | Primitive connected |
//! | `system.stopped.<name>` | [`SystemEventPayload`] | Primitive disconnected |
//! | `system.error.<name>` | [`SystemEventPayload`] | Primitive failed |
//! | `system.shutdown` | [`SystemShutdownPayload`] | Graceful shutdown signal |
//!
//! # Example
//!
//! ```rust
//! use emergent_client::prelude::*;
//! use emergent_client::types::{SystemEventPayload, SystemShutdownPayload};
//!
//! // Deserialize a system.started payload
//! # fn example() -> Result<(), serde_json::Error> {
//! # let json = r#"{"name":"timer","kind":"source","pid":1234,"publishes":["timer.tick"],"subscribes":[]}"#;
//! let payload: SystemEventPayload = serde_json::from_str(json)?;
//! println!("Started: {} ({})", payload.name(), payload.kind());
//!
//! // Check for error messages
//! if let Some(error) = payload.error() {
//!     eprintln!("Error: {}", error);
//! }
//! # Ok(())
//! # }
//! ```

use serde::{Deserialize, Serialize};
use std::fmt;

/// Payload for system lifecycle events (`system.started.*`, `system.stopped.*`, `system.error.*`).
///
/// This struct provides typed access to the information broadcast by the
/// Emergent engine when primitives start, stop, or encounter errors.
///
/// # Fields
///
/// - `name` - The name of the primitive (e.g., "timer", "filter")
/// - `kind` - The type of primitive: "source", "handler", or "sink"
/// - `pid` - Process ID if the primitive is running
/// - `publishes` - Message types this primitive publishes (Sources and Handlers)
/// - `subscribes` - Message types this primitive subscribes to (Handlers and Sinks)
/// - `error` - Error message if this is an error event
///
/// # Example
///
/// ```rust
/// use emergent_client::types::SystemEventPayload;
///
/// # fn example() -> Result<(), serde_json::Error> {
/// let json = r#"{
///     "name": "my-handler",
///     "kind": "handler",
///     "pid": 12345,
///     "publishes": ["output.enriched"],
///     "subscribes": ["input.event"]
/// }"#;
///
/// let payload: SystemEventPayload = serde_json::from_str(json)?;
/// assert_eq!(payload.name(), "my-handler");
/// assert_eq!(payload.kind(), "handler");
/// assert!(payload.is_handler());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemEventPayload {
    /// Name of the primitive.
    name: String,
    /// Kind of the primitive (source, handler, sink).
    kind: String,
    /// Process ID if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    /// Message types this primitive publishes (Sources and Handlers).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    publishes: Vec<String>,
    /// Message types this primitive subscribes to (Handlers and Sinks).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    subscribes: Vec<String>,
    /// Optional error message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl SystemEventPayload {
    /// Returns the name of the primitive.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the kind of the primitive ("source", "handler", or "sink").
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Returns the process ID if available.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Returns the message types this primitive publishes.
    ///
    /// For Sources and Handlers, this contains the declared publish types.
    /// For Sinks, this is always empty.
    #[must_use]
    pub fn publishes(&self) -> &[String] {
        &self.publishes
    }

    /// Returns the message types this primitive subscribes to.
    ///
    /// For Handlers and Sinks, this contains the declared subscription types.
    /// For Sources, this is always empty.
    #[must_use]
    pub fn subscribes(&self) -> &[String] {
        &self.subscribes
    }

    /// Returns the error message if this is an error event.
    ///
    /// This is `Some` for `system.error.*` events and `None` for
    /// `system.started.*` and `system.stopped.*` events.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Returns `true` if this primitive is a Source.
    #[must_use]
    pub fn is_source(&self) -> bool {
        self.kind == "source"
    }

    /// Returns `true` if this primitive is a Handler.
    #[must_use]
    pub fn is_handler(&self) -> bool {
        self.kind == "handler"
    }

    /// Returns `true` if this primitive is a Sink.
    #[must_use]
    pub fn is_sink(&self) -> bool {
        self.kind == "sink"
    }

    /// Returns `true` if this is an error event.
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }
}

impl fmt::Display for SystemEventPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.name, self.kind)?;
        if let Some(pid) = self.pid {
            write!(f, " [pid: {}]", pid)?;
        }
        if let Some(error) = &self.error {
            write!(f, " error: {}", error)?;
        }
        Ok(())
    }
}

/// Payload for system shutdown events (`system.shutdown`).
///
/// This struct represents the payload broadcast by the Emergent engine
/// when initiating a graceful shutdown sequence.
///
/// # Fields
///
/// - `kind` - The target primitive kind being shut down: "handler" or "sink"
///
/// The shutdown sequence proceeds in phases:
/// 1. Sources are stopped first (via SIGTERM)
/// 2. Handlers receive `system.shutdown` with `kind: "handler"`
/// 3. Sinks receive `system.shutdown` with `kind: "sink"`
///
/// # Example
///
/// ```rust
/// use emergent_client::types::SystemShutdownPayload;
///
/// # fn example() -> Result<(), serde_json::Error> {
/// let json = r#"{"kind": "handler"}"#;
/// let payload: SystemShutdownPayload = serde_json::from_str(json)?;
/// assert_eq!(payload.kind(), "handler");
/// assert!(payload.is_handler_shutdown());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemShutdownPayload {
    /// The target primitive kind being shut down ("handler" or "sink").
    kind: String,
}

impl SystemShutdownPayload {
    /// Returns the target primitive kind ("handler" or "sink").
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Returns `true` if this shutdown targets Handlers.
    #[must_use]
    pub fn is_handler_shutdown(&self) -> bool {
        self.kind == "handler"
    }

    /// Returns `true` if this shutdown targets Sinks.
    #[must_use]
    pub fn is_sink_shutdown(&self) -> bool {
        self.kind == "sink"
    }
}

impl fmt::Display for SystemShutdownPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "shutdown ({})", self.kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // SystemEventPayload Tests
    // ========================================================================

    #[test]
    fn deserialize_started_source() {
        let json = r#"{
            "name": "timer",
            "kind": "source",
            "pid": 1234,
            "publishes": ["timer.tick"],
            "subscribes": []
        }"#;

        let payload: SystemEventPayload = serde_json::from_str(json).expect("valid json");

        assert_eq!(payload.name(), "timer");
        assert_eq!(payload.kind(), "source");
        assert_eq!(payload.pid(), Some(1234));
        assert_eq!(payload.publishes(), &["timer.tick"]);
        assert!(payload.subscribes().is_empty());
        assert!(payload.error().is_none());
        assert!(payload.is_source());
        assert!(!payload.is_handler());
        assert!(!payload.is_sink());
        assert!(!payload.is_error());
    }

    #[test]
    fn deserialize_started_handler() {
        let json = r#"{
            "name": "filter",
            "kind": "handler",
            "pid": 5678,
            "publishes": ["timer.filtered"],
            "subscribes": ["timer.tick"]
        }"#;

        let payload: SystemEventPayload = serde_json::from_str(json).expect("valid json");

        assert_eq!(payload.name(), "filter");
        assert_eq!(payload.kind(), "handler");
        assert_eq!(payload.pid(), Some(5678));
        assert_eq!(payload.publishes(), &["timer.filtered"]);
        assert_eq!(payload.subscribes(), &["timer.tick"]);
        assert!(payload.is_handler());
    }

    #[test]
    fn deserialize_started_sink() {
        let json = r#"{
            "name": "console",
            "kind": "sink",
            "pid": 9012,
            "subscribes": ["timer.filtered"]
        }"#;

        let payload: SystemEventPayload = serde_json::from_str(json).expect("valid json");

        assert_eq!(payload.name(), "console");
        assert_eq!(payload.kind(), "sink");
        assert_eq!(payload.pid(), Some(9012));
        assert!(payload.publishes().is_empty());
        assert_eq!(payload.subscribes(), &["timer.filtered"]);
        assert!(payload.is_sink());
    }

    #[test]
    fn deserialize_stopped_without_pid() {
        let json = r#"{
            "name": "timer",
            "kind": "source",
            "publishes": ["timer.tick"]
        }"#;

        let payload: SystemEventPayload = serde_json::from_str(json).expect("valid json");

        assert_eq!(payload.name(), "timer");
        assert!(payload.pid().is_none());
    }

    #[test]
    fn deserialize_error_event() {
        let json = r#"{
            "name": "failing-source",
            "kind": "source",
            "pid": 3456,
            "publishes": ["data.event"],
            "error": "Connection refused"
        }"#;

        let payload: SystemEventPayload = serde_json::from_str(json).expect("valid json");

        assert_eq!(payload.name(), "failing-source");
        assert_eq!(payload.error(), Some("Connection refused"));
        assert!(payload.is_error());
    }

    #[test]
    fn deserialize_minimal_payload() {
        // Only required fields
        let json = r#"{"name": "test", "kind": "source"}"#;

        let payload: SystemEventPayload = serde_json::from_str(json).expect("valid json");

        assert_eq!(payload.name(), "test");
        assert_eq!(payload.kind(), "source");
        assert!(payload.pid().is_none());
        assert!(payload.publishes().is_empty());
        assert!(payload.subscribes().is_empty());
        assert!(payload.error().is_none());
    }

    #[test]
    fn serialize_roundtrip() {
        let json = r#"{
            "name": "my-handler",
            "kind": "handler",
            "pid": 42,
            "publishes": ["output.event"],
            "subscribes": ["input.event"]
        }"#;

        let payload: SystemEventPayload = serde_json::from_str(json).expect("valid json");
        let serialized = serde_json::to_string(&payload).expect("serialize");
        let restored: SystemEventPayload = serde_json::from_str(&serialized).expect("deserialize");

        assert_eq!(payload, restored);
    }

    #[test]
    fn display_format_basic() {
        let json = r#"{"name": "timer", "kind": "source", "pid": 1234}"#;
        let payload: SystemEventPayload = serde_json::from_str(json).expect("valid json");

        let display = payload.to_string();
        assert_eq!(display, "timer (source) [pid: 1234]");
    }

    #[test]
    fn display_format_without_pid() {
        let json = r#"{"name": "timer", "kind": "source"}"#;
        let payload: SystemEventPayload = serde_json::from_str(json).expect("valid json");

        let display = payload.to_string();
        assert_eq!(display, "timer (source)");
    }

    #[test]
    fn display_format_with_error() {
        let json = r#"{"name": "failing", "kind": "source", "error": "Connection refused"}"#;
        let payload: SystemEventPayload = serde_json::from_str(json).expect("valid json");

        let display = payload.to_string();
        assert_eq!(display, "failing (source) error: Connection refused");
    }

    #[test]
    fn payload_is_clone() {
        let json = r#"{"name": "timer", "kind": "source"}"#;
        let payload: SystemEventPayload = serde_json::from_str(json).expect("valid json");
        let cloned = payload.clone();

        assert_eq!(payload, cloned);
    }

    // ========================================================================
    // SystemShutdownPayload Tests
    // ========================================================================

    #[test]
    fn deserialize_handler_shutdown() {
        let json = r#"{"kind": "handler"}"#;

        let payload: SystemShutdownPayload = serde_json::from_str(json).expect("valid json");

        assert_eq!(payload.kind(), "handler");
        assert!(payload.is_handler_shutdown());
        assert!(!payload.is_sink_shutdown());
    }

    #[test]
    fn deserialize_sink_shutdown() {
        let json = r#"{"kind": "sink"}"#;

        let payload: SystemShutdownPayload = serde_json::from_str(json).expect("valid json");

        assert_eq!(payload.kind(), "sink");
        assert!(!payload.is_handler_shutdown());
        assert!(payload.is_sink_shutdown());
    }

    #[test]
    fn shutdown_serialize_roundtrip() {
        let json = r#"{"kind": "handler"}"#;

        let payload: SystemShutdownPayload = serde_json::from_str(json).expect("valid json");
        let serialized = serde_json::to_string(&payload).expect("serialize");
        let restored: SystemShutdownPayload =
            serde_json::from_str(&serialized).expect("deserialize");

        assert_eq!(payload, restored);
    }

    #[test]
    fn shutdown_display_format() {
        let json = r#"{"kind": "handler"}"#;
        let payload: SystemShutdownPayload = serde_json::from_str(json).expect("valid json");

        let display = payload.to_string();
        assert_eq!(display, "shutdown (handler)");
    }

    #[test]
    fn shutdown_is_clone() {
        let json = r#"{"kind": "sink"}"#;
        let payload: SystemShutdownPayload = serde_json::from_str(json).expect("valid json");
        let cloned = payload.clone();

        assert_eq!(payload, cloned);
    }
}
