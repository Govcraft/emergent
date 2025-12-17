//! Event storage for logging and replay.
//!
//! The event store provides two storage backends:
//! - JSON log files for human-readable streaming logs
//! - SQLite database for structured storage and replay

pub mod json_log;
pub mod sqlite;

pub use json_log::JsonEventLog;
pub use sqlite::SqliteEventStore;

use crate::messages::EmergentMessage;
use thiserror::Error;

/// Event store errors.
#[derive(Debug, Error)]
pub enum EventStoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Event store not initialized")]
    NotInitialized,
}

/// Trait for event storage backends.
pub trait EventStore: Send + Sync {
    /// Store an event.
    fn store(&self, message: &EmergentMessage) -> Result<(), EventStoreError>;

    /// Flush any buffered data.
    fn flush(&self) -> Result<(), EventStoreError>;
}
