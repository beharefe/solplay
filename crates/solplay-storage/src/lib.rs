//! SQLite persistence for normalized events and delivery attempts.

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use solplay_core::SolplayEvent;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("event serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("stored event `{id}` could not be decoded: {source}")]
    Deserialize {
        id: String,
        source: serde_json::Error,
    },
}

pub struct Database {
    connection: Connection,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let database = Self { connection };
        database.migrate()?;
        Ok(database)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        let connection = Connection::open_in_memory()?;
        let database = Self { connection };
        database.migrate()?;
        Ok(database)
    }

    pub fn insert_event(
        &self,
        event: &SolplayEvent,
        origin_key: &str,
    ) -> Result<bool, StorageError> {
        let payload = serde_json::to_string(event)?;
        let changed = self.connection.execute(
            "INSERT INTO events (id, origin_key, created_at, event_type, signature, source_address, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(origin_key) DO NOTHING",
            params![
                event.id,
                origin_key,
                event.created_at.to_rfc3339(),
                serde_json::to_string(&event.event_type)?,
                event.signature,
                event.source.address,
                payload,
            ],
        )?;
        Ok(changed == 1)
    }

    pub fn events(
        &self,
        event_type: Option<&str>,
        failed_only: bool,
    ) -> Result<Vec<StoredEvent>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT e.id, e.created_at, e.event_type, e.payload,
                    EXISTS(SELECT 1 FROM delivery_attempts d WHERE d.event_id = e.id AND d.status = 'failed')
             FROM events e
             WHERE (?1 IS NULL OR e.event_type = ?1)
               AND (?2 = 0 OR EXISTS(SELECT 1 FROM delivery_attempts d WHERE d.event_id = e.id AND d.status = 'failed'))
             ORDER BY e.created_at DESC",
        )?;
        let rows = statement.query_map(params![event_type, i64::from(failed_only)], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })?;

        rows.map(|row| {
            let (id, created_at, event_type, payload, has_failed_delivery) = row?;
            let event =
                serde_json::from_str(&payload).map_err(|source| StorageError::Deserialize {
                    id: id.clone(),
                    source,
                })?;
            let created_at = DateTime::parse_from_rfc3339(&created_at)
                .map_err(|error| StorageError::Deserialize {
                    id: id.clone(),
                    source: serde_json::Error::io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        error,
                    )),
                })?
                .with_timezone(&Utc);
            Ok(StoredEvent {
                id,
                created_at,
                event_type,
                event,
                has_failed_delivery,
            })
        })
        .collect()
    }

    pub fn record_delivery(
        &self,
        event_id: &str,
        destination_name: &str,
        result: &DeliveryAttempt,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO delivery_attempts (event_id, destination_name, status, status_code, duration_ms, response, attempted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![event_id, destination_name, result.status, result.status_code, result.duration_ms, result.response, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn monitor_summary(&self) -> Result<MonitorSummary, StorageError> {
        let events = self
            .connection
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
        let delivered = self.connection.query_row(
            "SELECT COUNT(*) FROM delivery_attempts WHERE status = 'delivered'",
            [],
            |row| row.get(0),
        )?;
        let failed = self.connection.query_row(
            "SELECT COUNT(*) FROM delivery_attempts WHERE status = 'failed'",
            [],
            |row| row.get(0),
        )?;
        Ok(MonitorSummary {
            events,
            delivered,
            failed,
        })
    }

    fn migrate(&self) -> Result<(), StorageError> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                id TEXT PRIMARY KEY,
                origin_key TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL,
                event_type TEXT NOT NULL,
                signature TEXT NOT NULL,
                source_address TEXT NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS events_created_at_idx ON events(created_at DESC);
            CREATE INDEX IF NOT EXISTS events_type_idx ON events(event_type);
            CREATE TABLE IF NOT EXISTS delivery_attempts (
                id INTEGER PRIMARY KEY,
                event_id TEXT NOT NULL REFERENCES events(id),
                destination_name TEXT NOT NULL,
                status TEXT NOT NULL,
                status_code INTEGER,
                duration_ms INTEGER NOT NULL,
                response TEXT,
                attempted_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS delivery_attempts_event_idx ON delivery_attempts(event_id);
            CREATE INDEX IF NOT EXISTS delivery_attempts_status_idx ON delivery_attempts(status);",
        )?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct MonitorSummary {
    pub events: u64,
    pub delivered: u64,
    pub failed: u64,
}

#[derive(Debug)]
pub struct StoredEvent {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub event_type: String,
    pub event: SolplayEvent,
    pub has_failed_delivery: bool,
}

#[derive(Debug)]
pub struct DeliveryAttempt {
    pub status: &'static str,
    pub status_code: Option<u16>,
    pub duration_ms: u64,
    pub response: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rust_decimal::Decimal;
    use solplay_core::{EventData, EventType, SourceAddress};

    use super::*;

    #[test]
    fn origin_key_makes_event_insertion_idempotent() {
        let database = Database::open_in_memory().expect("database opens");
        let source =
            SourceAddress::from_str("11111111111111111111111111111111").expect("valid source");
        let event = SolplayEvent::new(
            EventType::TokenReceived,
            1,
            "signature",
            &source,
            EventData {
                mint: None,
                amount: Some(Decimal::ONE),
            },
        );

        assert!(
            database
                .insert_event(&event, "signature:0")
                .expect("first insert succeeds")
        );
        assert!(
            !database
                .insert_event(&event, "signature:0")
                .expect("second insert is ignored")
        );
    }
}
