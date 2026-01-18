//! JSON log file event storage.
//!
//! Provides append-only JSON lines format logging for human-readable
//! event streams and debugging.

use super::{EventStore, EventStoreError};
use crate::messages::EmergentMessage;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A single log entry in the JSON log file.
#[derive(Serialize)]
struct LogEntry<'a> {
    /// ISO 8601 timestamp.
    timestamp: String,
    /// The message being logged.
    message: &'a EmergentMessage,
}

/// JSON event log storage.
///
/// Writes events to JSON lines format files, one event per line.
/// Creates new log files based on date for rotation.
pub struct JsonEventLog {
    /// Directory for log files.
    log_dir: PathBuf,
    /// Current log file writer.
    writer: Mutex<Option<BufWriter<File>>>,
    /// Current log file date (for rotation).
    current_date: Mutex<Option<String>>,
}

impl JsonEventLog {
    /// Create a new JSON event log.
    ///
    /// # Errors
    ///
    /// Returns an error if the log directory cannot be created.
    pub fn new<P: AsRef<Path>>(log_dir: P) -> Result<Self, EventStoreError> {
        let log_dir = log_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&log_dir)?;

        Ok(Self {
            log_dir,
            writer: Mutex::new(None),
            current_date: Mutex::new(None),
        })
    }

    /// Get or create the log file for the current date.
    fn get_writer(&self) -> Result<std::sync::MutexGuard<'_, Option<BufWriter<File>>>, EventStoreError> {
        let today = Utc::now().format("%Y-%m-%d").to_string();

        let mut writer = self.writer.lock().map_err(|e| {
            EventStoreError::LockPoisoned(format!("writer lock: {e}"))
        })?;
        let mut current_date = self.current_date.lock().map_err(|e| {
            EventStoreError::LockPoisoned(format!("current_date lock: {e}"))
        })?;

        // Check if we need to rotate to a new file
        let needs_rotation = current_date.as_ref() != Some(&today);

        if needs_rotation {
            // Flush existing writer
            if let Some(ref mut w) = *writer {
                w.flush()?;
            }

            // Create new log file
            let log_path = self.log_dir.join(format!("events-{today}.jsonl"));
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)?;

            *writer = Some(BufWriter::new(file));
            *current_date = Some(today);
        }

        drop(current_date);
        Ok(writer)
    }
}

impl EventStore for JsonEventLog {
    fn store(&self, message: &EmergentMessage) -> Result<(), EventStoreError> {
        let timestamp: DateTime<Utc> = DateTime::from_timestamp_millis(
            i64::try_from(message.timestamp_ms.as_millis()).unwrap_or(i64::MAX),
        )
        .unwrap_or_else(Utc::now);

        let entry = LogEntry {
            timestamp: timestamp.to_rfc3339(),
            message,
        };

        let mut writer = self.get_writer()?;
        if let Some(ref mut w) = *writer {
            serde_json::to_writer(&mut *w, &entry)?;
            writeln!(w)?;
        }

        Ok(())
    }

    fn flush(&self) -> Result<(), EventStoreError> {
        let mut writer = self.writer.lock().map_err(|e| {
            EventStoreError::LockPoisoned(format!("writer lock: {e}"))
        })?;
        if let Some(ref mut w) = *writer {
            w.flush()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn test_json_log_write() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let log = JsonEventLog::new(temp_dir.path())?;

        let message = EmergentMessage::new("test.event")
            .with_source("test")
            .with_payload(json!({"key": "value"}));

        log.store(&message)?;
        log.flush()?;

        // Verify file was created
        let entries: Vec<_> = std::fs::read_dir(temp_dir.path())?.collect();
        assert_eq!(entries.len(), 1);
        Ok(())
    }
}
