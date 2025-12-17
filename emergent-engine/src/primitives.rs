//! Primitive types for Sources, Handlers, and Sinks.
//!
//! These types define the state and metadata for the three core primitives
//! that make up the Emergent workflow system.

use serde::{Deserialize, Serialize};

/// The kind of primitive (Source, Handler, or Sink).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrimitiveKind {
    /// A Source publishes messages (ingress from external world).
    Source,
    /// A Handler subscribes to and publishes messages (transformation).
    Handler,
    /// A Sink subscribes to messages (egress to external world).
    Sink,
}

impl std::fmt::Display for PrimitiveKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source => write!(f, "Source"),
            Self::Handler => write!(f, "Handler"),
            Self::Sink => write!(f, "Sink"),
        }
    }
}

/// The lifecycle state of a primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrimitiveState {
    /// The primitive is configured but not yet started.
    Configured,
    /// The primitive is in the process of starting.
    Starting,
    /// The primitive is running and processing messages.
    Running,
    /// The primitive is in the process of stopping.
    Stopping,
    /// The primitive has stopped gracefully.
    Stopped,
    /// The primitive has failed and is not running.
    Failed,
    /// The primitive is an external connection (not managed by engine).
    External,
}

impl std::fmt::Display for PrimitiveState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configured => write!(f, "Configured"),
            Self::Starting => write!(f, "Starting"),
            Self::Running => write!(f, "Running"),
            Self::Stopping => write!(f, "Stopping"),
            Self::Stopped => write!(f, "Stopped"),
            Self::Failed => write!(f, "Failed"),
            Self::External => write!(f, "External"),
        }
    }
}

/// Information about a registered primitive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrimitiveInfo {
    /// Unique name of the primitive.
    pub name: String,

    /// The kind of primitive.
    pub kind: PrimitiveKind,

    /// Current state of the primitive.
    pub state: PrimitiveState,

    /// Message types this primitive publishes (Sources and Handlers).
    pub publishes: Vec<String>,

    /// Message types this primitive subscribes to (Handlers and Sinks).
    pub subscribes: Vec<String>,

    /// Process ID if managed by the engine.
    pub pid: Option<u32>,

    /// Error message if the primitive is in Failed state.
    pub error: Option<String>,
}

impl PrimitiveInfo {
    /// Create a new primitive info for a Source.
    #[must_use]
    pub fn source(name: &str, publishes: Vec<String>) -> Self {
        Self {
            name: name.to_string(),
            kind: PrimitiveKind::Source,
            state: PrimitiveState::Configured,
            publishes,
            subscribes: Vec::new(),
            pid: None,
            error: None,
        }
    }

    /// Create a new primitive info for a Handler.
    #[must_use]
    pub fn handler(name: &str, subscribes: Vec<String>, publishes: Vec<String>) -> Self {
        Self {
            name: name.to_string(),
            kind: PrimitiveKind::Handler,
            state: PrimitiveState::Configured,
            publishes,
            subscribes,
            pid: None,
            error: None,
        }
    }

    /// Create a new primitive info for a Sink.
    #[must_use]
    pub fn sink(name: &str, subscribes: Vec<String>) -> Self {
        Self {
            name: name.to_string(),
            kind: PrimitiveKind::Sink,
            state: PrimitiveState::Configured,
            publishes: Vec::new(),
            subscribes,
            pid: None,
            error: None,
        }
    }

    /// Check if this primitive is running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(self.state, PrimitiveState::Running | PrimitiveState::External)
    }

    /// Check if this primitive can be started.
    #[must_use]
    pub fn can_start(&self) -> bool {
        matches!(
            self.state,
            PrimitiveState::Configured | PrimitiveState::Stopped | PrimitiveState::Failed
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_primitive_kind_display() {
        assert_eq!(PrimitiveKind::Source.to_string(), "Source");
        assert_eq!(PrimitiveKind::Handler.to_string(), "Handler");
        assert_eq!(PrimitiveKind::Sink.to_string(), "Sink");
    }

    #[test]
    fn test_primitive_state_transitions() {
        let mut info = PrimitiveInfo::source("test", vec!["test.event".to_string()]);

        assert!(info.can_start());
        assert!(!info.is_running());

        info.state = PrimitiveState::Running;
        assert!(!info.can_start());
        assert!(info.is_running());

        info.state = PrimitiveState::Failed;
        assert!(info.can_start());
        assert!(!info.is_running());
    }
}
