//! SQLite event storage for structured queries and replay.
//!
//! Provides persistent storage with querying capabilities for
//! replaying events by time range, message type, or source.

use super::{EventStore, EventStoreError};
use crate::messages::{
    CausationId, CorrelationId, EmergentMessage, MessageId, MessageType, PrimitiveName, Timestamp,
};
use rusqlite::{Connection, params};
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
        configure_journal(&conn)?;

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
    pub fn query_by_type(
        &self,
        message_type: &str,
    ) -> Result<Vec<EmergentMessage>, EventStoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| EventStoreError::LockPoisoned(format!("conn lock: {e}")))?;
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
        let conn = self
            .conn
            .lock()
            .map_err(|e| EventStoreError::LockPoisoned(format!("conn lock: {e}")))?;
        let mut stmt = conn.prepare(
            r"
            SELECT id, message_type, source, correlation_id, causation_id,
                   timestamp_ms, payload_json, metadata_json
            FROM events
            WHERE timestamp_ms >= ? AND timestamp_ms <= ?
            ORDER BY timestamp_ms ASC
            ",
        )?;

        let events = stmt.query_map([start_ms, end_ms], row_to_message)?;

        events.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Query events by correlation ID (for tracing message chains).
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub fn query_by_correlation(
        &self,
        correlation_id: &str,
    ) -> Result<Vec<EmergentMessage>, EventStoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| EventStoreError::LockPoisoned(format!("conn lock: {e}")))?;
        let mut stmt = conn.prepare(
            r"
            SELECT id, message_type, source, correlation_id, causation_id,
                   timestamp_ms, payload_json, metadata_json
            FROM events
            WHERE correlation_id = ?
            ORDER BY timestamp_ms ASC
            ",
        )?;

        let events = stmt.query_map([correlation_id], row_to_message)?;

        events.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get the count of events in the store.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub fn count(&self) -> Result<u64, EventStoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| EventStoreError::LockPoisoned(format!("conn lock: {e}")))?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    /// Delete events older than the specified timestamp.
    ///
    /// # Errors
    ///
    /// Returns an error if the deletion fails.
    pub fn delete_before(&self, timestamp_ms: u64) -> Result<u64, EventStoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| EventStoreError::LockPoisoned(format!("conn lock: {e}")))?;
        let deleted = conn.execute("DELETE FROM events WHERE timestamp_ms < ?", [timestamp_ms])?;
        Ok(u64::try_from(deleted).unwrap_or(0))
    }
}

/// Put a file-backed connection in write-ahead-log mode.
///
/// In SQLite's default rollback-journal mode a reader holds a shared lock on
/// the whole file, so anything tailing `events.db` (`sqlite3`, a dashboard, a
/// control plane polling for new events) makes the engine's next insert fail
/// with `SQLITE_BUSY` for as long as its read lasts. In WAL mode readers see
/// the last committed snapshot and never block the writer, nor it them.
///
/// `synchronous = NORMAL` is the setting SQLite recommends with WAL: a commit
/// no longer waits for an fsync, only a checkpoint does. The database cannot be
/// corrupted by a crash or power loss either way; what `NORMAL` gives up is the
/// last few commits on a power loss or OS crash (not on an engine crash, which
/// loses nothing). The event store is an audit and replay log written next to
/// the JSON log, which is not fsynced per event either, so this matches the
/// durability the engine already offers. `journal_mode` is stored in the
/// database file and persists; `synchronous` is per connection, which is why it
/// is set on every open.
///
/// # Errors
///
/// Returns an error if either pragma cannot be applied.
fn configure_journal(conn: &Connection) -> Result<(), rusqlite::Error> {
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        tracing::warn!(
            journal_mode = %mode,
            "SQLite event store could not switch to WAL mode; readers may block event writes"
        );
    }
    conn.execute_batch("PRAGMA synchronous = NORMAL;")?;
    Ok(())
}

fn row_to_message(row: &rusqlite::Row<'_>) -> Result<EmergentMessage, rusqlite::Error> {
    let id_str: String = row.get(0)?;
    let message_type_str: String = row.get(1)?;
    let source_str: String = row.get(2)?;
    let correlation_id_str: Option<String> = row.get(3)?;
    let causation_id_str: Option<String> = row.get(4)?;
    let timestamp_ms: i64 = row.get(5)?;
    let payload_json: String = row.get(6)?;
    let metadata_json: Option<String> = row.get(7)?;

    // Parse newtypes from strings
    let id = MessageId::parse(&id_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let message_type = MessageType::new(&message_type_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let source = PrimitiveName::new(&source_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let correlation_id = correlation_id_str
        .map(|s| {
            CorrelationId::parse(&s).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })
        })
        .transpose()?;

    let causation_id = causation_id_str
        .map(|s| {
            CausationId::parse(&s).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })
        })
        .transpose()?;

    let timestamp = Timestamp::from_millis(u64::try_from(timestamp_ms).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Integer, Box::new(e))
    })?);

    Ok(EmergentMessage {
        id,
        message_type,
        source,
        correlation_id,
        causation_id,
        timestamp_ms: timestamp,
        payload: serde_json::from_str(&payload_json).unwrap_or(serde_json::Value::Null),
        metadata: metadata_json.and_then(|s| serde_json::from_str(&s).ok()),
    })
}

impl EventStore for SqliteEventStore {
    fn store(&self, message: &EmergentMessage) -> Result<(), EventStoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| EventStoreError::LockPoisoned(format!("conn lock: {e}")))?;

        let payload_json = serde_json::to_string(&message.payload)?;
        let metadata_json = message
            .metadata
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        // Convert newtypes to strings for storage
        let id_str = message.id.to_string();
        let message_type_str = message.message_type.as_str();
        let source_str = message.source.as_str();
        let correlation_id_str = message.correlation_id.as_ref().map(ToString::to_string);
        let causation_id_str = message.causation_id.as_ref().map(ToString::to_string);

        let timestamp_ms = i64::try_from(message.timestamp_ms.as_millis())
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

        conn.execute(
            r"
            INSERT OR REPLACE INTO events
            (id, message_type, source, correlation_id, causation_id, timestamp_ms, payload_json, metadata_json)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ",
            params![
                id_str,
                message_type_str,
                source_str,
                correlation_id_str,
                causation_id_str,
                timestamp_ms,
                payload_json,
                metadata_json,
            ],
        )?;

        Ok(())
    }

    fn flush(&self) -> Result<(), EventStoreError> {
        // SQLite commits are automatic, but we can force a checkpoint
        let conn = self
            .conn
            .lock()
            .map_err(|e| EventStoreError::LockPoisoned(format!("conn lock: {e}")))?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_sqlite_store_and_query() -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteEventStore::in_memory()?;

        let msg1 = EmergentMessage::new("timer.tick")
            .with_source("timer")
            .with_payload(json!({"seq": 1}));

        let msg2 = EmergentMessage::new("timer.tick")
            .with_source("timer")
            .with_payload(json!({"seq": 2}));

        let msg3 = EmergentMessage::new("other.event")
            .with_source("other")
            .with_payload(json!({}));

        store.store(&msg1)?;
        store.store(&msg2)?;
        store.store(&msg3)?;

        assert_eq!(store.count()?, 3);

        let timer_events = store.query_by_type("timer.tick")?;
        assert_eq!(timer_events.len(), 2);
        Ok(())
    }

    #[test]
    fn a_file_store_uses_wal_with_normal_sync() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteEventStore::new(dir.path().join("events.db"))?;
        let conn = store.conn.lock().map_err(|e| format!("conn lock: {e}"))?;

        let mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        assert_eq!(mode.to_ascii_lowercase(), "wal");

        // 1 is NORMAL (0 OFF, 2 FULL, 3 EXTRA).
        let synchronous: i64 = conn.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
        assert_eq!(synchronous, 1);
        Ok(())
    }

    #[test]
    fn an_open_reader_does_not_block_a_write() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("events.db");
        let store = SqliteEventStore::new(&path)?;
        store.store(&EmergentMessage::new("timer.tick").with_source("timer"))?;

        // An external reader, such as a control plane tailing events, holding
        // a read transaction open. In rollback-journal mode its shared lock
        // makes the insert below fail with SQLITE_BUSY.
        let reader = Connection::open(&path)?;
        reader.execute_batch("BEGIN")?;
        let seen: i64 = reader.query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
        assert_eq!(seen, 1);

        store.store(&EmergentMessage::new("timer.tick").with_source("timer"))?;
        assert_eq!(store.count()?, 2);

        // The reader keeps its snapshot until it ends the transaction.
        let still: i64 = reader.query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
        assert_eq!(still, 1);
        reader.execute_batch("COMMIT")?;
        let now: i64 = reader.query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
        assert_eq!(now, 2);
        Ok(())
    }

    #[test]
    fn test_correlation_query() -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteEventStore::in_memory()?;

        // Create a shared correlation ID for both messages
        let correlation_id = CorrelationId::new();
        let correlation_id_str = correlation_id.to_string();

        let msg1 = EmergentMessage::new("request")
            .with_source("client")
            .with_correlation_id(correlation_id.clone());

        let msg2 = EmergentMessage::new("response")
            .with_source("server")
            .with_correlation_id(correlation_id);

        store.store(&msg1)?;
        store.store(&msg2)?;

        let correlated = store.query_by_correlation(&correlation_id_str)?;
        assert_eq!(correlated.len(), 2);
        Ok(())
    }
}
