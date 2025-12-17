//! SQLite event storage for structured queries and replay.
//!
//! Provides persistent storage with querying capabilities for
//! replaying events by time range, message type, or source.

use super::{EventStore, EventStoreError};
use crate::messages::EmergentMessage;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

/// SQLite event store.
///
/// Stores events in a SQLite database for structured queries and replay.
pub struct SqliteEventStore {
    /// Database connection.
    conn: Mutex<Connection>,
}

impl SqliteEventStore {
    /// Create a new SQLite event store.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be opened or initialized.
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, EventStoreError> {
        let conn = Connection::open(db_path)?;

        // Create tables
        conn.execute_batch(
            r"
            CREATE TABLE IF NOT EXISTS events (
                id TEXT PRIMARY KEY,
                message_type TEXT NOT NULL,
                source TEXT NOT NULL,
                correlation_id TEXT,
                causation_id TEXT,
                timestamp_ms INTEGER NOT NULL,
                payload_json TEXT NOT NULL,
                metadata_json TEXT,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_events_message_type ON events(message_type);
            CREATE INDEX IF NOT EXISTS idx_events_timestamp ON events(timestamp_ms);
            CREATE INDEX IF NOT EXISTS idx_events_source ON events(source);
            CREATE INDEX IF NOT EXISTS idx_events_correlation ON events(correlation_id);
            ",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Create an in-memory SQLite event store (for testing).
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be initialized.
    pub fn in_memory() -> Result<Self, EventStoreError> {
        let conn = Connection::open_in_memory()?;

        conn.execute_batch(
            r"
            CREATE TABLE events (
                id TEXT PRIMARY KEY,
                message_type TEXT NOT NULL,
                source TEXT NOT NULL,
                correlation_id TEXT,
                causation_id TEXT,
                timestamp_ms INTEGER NOT NULL,
                payload_json TEXT NOT NULL,
                metadata_json TEXT,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX idx_events_message_type ON events(message_type);
            CREATE INDEX idx_events_timestamp ON events(timestamp_ms);
            ",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Query events by message type.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub fn query_by_type(&self, message_type: &str) -> Result<Vec<EmergentMessage>, EventStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r"
            SELECT id, message_type, source, correlation_id, causation_id,
                   timestamp_ms, payload_json, metadata_json
            FROM events
            WHERE message_type = ?
            ORDER BY timestamp_ms ASC
            ",
        )?;

        let events = stmt.query_map([message_type], row_to_message)?;

        events.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Query events by time range.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub fn query_by_time_range(
        &self,
        start_ms: u64,
        end_ms: u64,
    ) -> Result<Vec<EmergentMessage>, EventStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r"
            SELECT id, message_type, source, correlation_id, causation_id,
                   timestamp_ms, payload_json, metadata_json
            FROM events
            WHERE timestamp_ms >= ? AND timestamp_ms <= ?
            ORDER BY timestamp_ms ASC
            ",
        )?;

        let events = stmt.query_map([start_ms, end_ms], |row| {
            row_to_message(row)
        })?;

        events.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Query events by correlation ID (for tracing message chains).
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub fn query_by_correlation(&self, correlation_id: &str) -> Result<Vec<EmergentMessage>, EventStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r"
            SELECT id, message_type, source, correlation_id, causation_id,
                   timestamp_ms, payload_json, metadata_json
            FROM events
            WHERE correlation_id = ?
            ORDER BY timestamp_ms ASC
            ",
        )?;

        let events = stmt.query_map([correlation_id], |row| {
            row_to_message(row)
        })?;

        events.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get the count of events in the store.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub fn count(&self) -> Result<u64, EventStoreError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    /// Delete events older than the specified timestamp.
    ///
    /// # Errors
    ///
    /// Returns an error if the deletion fails.
    pub fn delete_before(&self, timestamp_ms: u64) -> Result<u64, EventStoreError> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute(
            "DELETE FROM events WHERE timestamp_ms < ?",
            [timestamp_ms],
        )?;
        Ok(u64::try_from(deleted).unwrap_or(0))
    }
}

fn row_to_message(row: &rusqlite::Row<'_>) -> Result<EmergentMessage, rusqlite::Error> {
    let payload_json: String = row.get(6)?;
    let metadata_json: Option<String> = row.get(7)?;

    Ok(EmergentMessage {
        id: row.get(0)?,
        message_type: row.get(1)?,
        source: row.get(2)?,
        correlation_id: row.get(3)?,
        causation_id: row.get(4)?,
        timestamp_ms: row.get::<_, i64>(5)? as u64,
        payload: serde_json::from_str(&payload_json).unwrap_or(serde_json::Value::Null),
        metadata: metadata_json.and_then(|s| serde_json::from_str(&s).ok()),
    })
}

impl EventStore for SqliteEventStore {
    fn store(&self, message: &EmergentMessage) -> Result<(), EventStoreError> {
        let conn = self.conn.lock().unwrap();

        let payload_json = serde_json::to_string(&message.payload)?;
        let metadata_json = message
            .metadata
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        conn.execute(
            r"
            INSERT OR REPLACE INTO events
            (id, message_type, source, correlation_id, causation_id, timestamp_ms, payload_json, metadata_json)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ",
            params![
                message.id,
                message.message_type,
                message.source,
                message.correlation_id,
                message.causation_id,
                message.timestamp_ms as i64,
                payload_json,
                metadata_json,
            ],
        )?;

        Ok(())
    }

    fn flush(&self) -> Result<(), EventStoreError> {
        // SQLite commits are automatic, but we can force a checkpoint
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_sqlite_store_and_query() {
        let store = SqliteEventStore::in_memory().unwrap();

        let msg1 = EmergentMessage::new("timer.tick")
            .with_source("timer")
            .with_payload(json!({"seq": 1}));

        let msg2 = EmergentMessage::new("timer.tick")
            .with_source("timer")
            .with_payload(json!({"seq": 2}));

        let msg3 = EmergentMessage::new("other.event")
            .with_source("other")
            .with_payload(json!({}));

        store.store(&msg1).unwrap();
        store.store(&msg2).unwrap();
        store.store(&msg3).unwrap();

        assert_eq!(store.count().unwrap(), 3);

        let timer_events = store.query_by_type("timer.tick").unwrap();
        assert_eq!(timer_events.len(), 2);
    }

    #[test]
    fn test_correlation_query() {
        let store = SqliteEventStore::in_memory().unwrap();

        let msg1 = EmergentMessage::new("request")
            .with_source("client")
            .with_correlation_id("corr_123");

        let msg2 = EmergentMessage::new("response")
            .with_source("server")
            .with_correlation_id("corr_123");

        store.store(&msg1).unwrap();
        store.store(&msg2).unwrap();

        let correlated = store.query_by_correlation("corr_123").unwrap();
        assert_eq!(correlated.len(), 2);
    }
}
