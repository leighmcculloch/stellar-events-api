use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::ledger::events::ExtractedEvent;
use crate::Error;

/// SQLite-backed event store.
pub struct EventStore {
    conn: Connection,
}

impl EventStore {
    /// Open or create the database at the given path.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Open an in-memory database (for testing).
    pub fn open_memory() -> Result<Self, Error> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), Error> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS events (
                id TEXT PRIMARY KEY,
                ledger_sequence INTEGER NOT NULL,
                tx_index INTEGER NOT NULL,
                event_index INTEGER NOT NULL,
                ledger_closed_at INTEGER NOT NULL,
                contract_id TEXT,
                event_type TEXT NOT NULL,
                topics_json TEXT NOT NULL,
                data_json TEXT NOT NULL,
                event_json TEXT NOT NULL,
                tx_hash TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_events_ledger ON events(ledger_sequence);
            CREATE INDEX IF NOT EXISTS idx_events_contract ON events(contract_id);
            CREATE INDEX IF NOT EXISTS idx_events_type ON events(event_type);
            CREATE INDEX IF NOT EXISTS idx_events_id ON events(id);

            CREATE TABLE IF NOT EXISTS ledger_cache (
                ledger_sequence INTEGER PRIMARY KEY,
                fetched_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sync_state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;
        Ok(())
    }

    /// Insert extracted events into the database.
    pub fn insert_events(&self, events: &[ExtractedEvent]) -> Result<(), Error> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO events (id, ledger_sequence, tx_index, event_index, ledger_closed_at, contract_id, event_type, topics_json, data_json, event_json, tx_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            for event in events {
                let id = crate::ledger::events::event_id(
                    event.ledger_sequence,
                    event.phase,
                    event.tx_index,
                    event.event_index,
                );
                let topics_json = serde_json::to_string(&event.topics_xdr_json)?;
                let data_json = serde_json::to_string(&event.data_xdr_json)?;
                let event_json = serde_json::to_string(&event.event_xdr_json)?;
                stmt.execute(params![
                    id,
                    event.ledger_sequence,
                    event.tx_index,
                    event.event_index,
                    event.ledger_closed_at,
                    event.contract_id,
                    event.event_type.to_string(),
                    topics_json,
                    data_json,
                    event_json,
                    event.tx_hash,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Record that a ledger has been cached.
    pub fn record_ledger_cached(
        &self,
        ledger_sequence: u32,
        ttl_seconds: i64,
    ) -> Result<(), Error> {
        let now = chrono::Utc::now().timestamp();
        self.conn.execute(
            "INSERT OR REPLACE INTO ledger_cache (ledger_sequence, fetched_at, expires_at) VALUES (?1, ?2, ?3)",
            params![ledger_sequence, now, now + ttl_seconds],
        )?;
        Ok(())
    }

    /// Check if a ledger is cached and not expired.
    pub fn is_ledger_cached(&self, ledger_sequence: u32) -> Result<bool, Error> {
        let now = chrono::Utc::now().timestamp();
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM ledger_cache WHERE ledger_sequence = ?1 AND expires_at > ?2",
            params![ledger_sequence, now],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Get or set sync state.
    pub fn get_sync_state(&self, key: &str) -> Result<Option<String>, Error> {
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM sync_state WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(result)
    }

    pub fn set_sync_state(&self, key: &str, value: &str) -> Result<(), Error> {
        self.conn.execute(
            "INSERT OR REPLACE INTO sync_state (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    /// Query events with cursor-based pagination and filtering.
    pub fn query_events(&self, params: &EventQueryParams) -> Result<EventQueryResult, Error> {
        let mut conditions = Vec::new();
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        // Cursor-based pagination
        if let Some(ref after) = params.after {
            conditions.push("id > ?".to_string());
            bind_values.push(Box::new(after.clone()));
        }

        // Ledger sequence filter — pin to a single ledger.
        // If `ledger` is set, use it. If neither cursor nor ledger is set, default
        // to the latest ledger in the database.
        let pinned_ledger = match (params.after.as_ref(), params.ledger) {
            (None, None) => {
                let max: Option<i64> = self
                    .conn
                    .query_row("SELECT MAX(ledger_sequence) FROM events", [], |row| {
                        row.get(0)
                    })
                    .optional()?
                    .flatten();
                max.map(|v| v as u32)
            }
            _ => params.ledger,
        };
        if let Some(seq) = pinned_ledger {
            conditions.push("ledger_sequence = ?".to_string());
            bind_values.push(Box::new(seq as i64));
        }

        // Transaction hash filter (top-level, limits entire result set)
        if let Some(ref tx_hash) = params.tx_hash {
            conditions.push("tx_hash = ?".to_string());
            bind_values.push(Box::new(tx_hash.clone()));
        }

        // Structured filters: each filter is OR'd; conditions within are AND'd
        if !params.filters.is_empty() {
            let mut filter_clauses = Vec::new();
            for filter in &params.filters {
                let mut fc = Vec::new();

                if let Some(ref cid) = filter.contract_id {
                    fc.push("contract_id = ?".to_string());
                    bind_values.push(Box::new(cid.clone()));
                }

                if let Some(ref et) = filter.event_type {
                    fc.push("event_type = ?".to_string());
                    bind_values.push(Box::new(et.clone()));
                }

                if let Some(ref topics) = filter.topics {
                    if !topics.is_empty() {
                        // Event must have at least as many topics as the filter specifies
                        fc.push(format!(
                            "json_array_length(topics_json) >= {}",
                            topics.len()
                        ));
                        for (i, topic) in topics.iter().enumerate() {
                            if topic.as_str() == Some("*") {
                                // Wildcard: position must exist (handled by length check)
                                continue;
                            }
                            let topic_json = serde_json::to_string(topic).unwrap_or_default();
                            fc.push(format!(
                                "json(json_extract(topics_json, '$[{}]')) = json(?)",
                                i
                            ));
                            bind_values.push(Box::new(topic_json));
                        }
                    }
                }

                if fc.is_empty() {
                    filter_clauses.push("1=1".to_string());
                } else {
                    filter_clauses.push(format!("({})", fc.join(" AND ")));
                }
            }
            conditions.push(format!("({})", filter_clauses.join(" OR ")));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        // Fetch one extra to determine has_more
        let fetch_limit = params.limit + 1;

        let sql = format!(
            "SELECT id, ledger_sequence, tx_index, event_index, ledger_closed_at, contract_id, event_type, topics_json, data_json, event_json, tx_hash FROM events {} ORDER BY id ASC LIMIT ?",
            where_clause
        );

        bind_values.push(Box::new(fetch_limit as i64));

        let mut stmt = self.conn.prepare(&sql)?;
        let bind_refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|v| v.as_ref()).collect();

        let rows = stmt.query_map(bind_refs.as_slice(), |row| {
            let topics_str: String = row.get(7)?;
            let data_str: String = row.get(8)?;
            let event_str: String = row.get(9)?;
            Ok(EventRow {
                id: row.get(0)?,
                ledger_sequence: row.get::<_, i64>(1)? as u32,
                tx_index: row.get::<_, i64>(2)? as u32,
                event_index: row.get::<_, i64>(3)? as u32,
                ledger_closed_at: row.get(4)?,
                contract_id: row.get(5)?,
                event_type: row.get(6)?,
                topics_json: topics_str,
                data_json: data_str,
                event_json: event_str,
                tx_hash: row.get(10)?,
            })
        })?;

        let mut results: Vec<EventRow> = rows.collect::<Result<Vec<_>, _>>()?;
        let has_more = results.len() > params.limit as usize;
        if has_more {
            results.truncate(params.limit as usize);
        }

        Ok(EventQueryResult {
            data: results,
            has_more,
        })
    }

    /// Get the highest ledger sequence in the database.
    pub fn latest_ledger_sequence(&self) -> Result<Option<u32>, Error> {
        let result: Option<i64> = self
            .conn
            .query_row("SELECT MAX(ledger_sequence) FROM ledger_cache", [], |row| {
                row.get(0)
            })
            .optional()?
            .flatten();
        Ok(result.map(|v| v as u32))
    }

    /// Get the lowest ledger sequence in the (non-expired) cache.
    pub fn earliest_ledger_sequence(&self) -> Result<Option<u32>, Error> {
        let now = chrono::Utc::now().timestamp();
        let result: Option<i64> = self
            .conn
            .query_row(
                "SELECT MIN(ledger_sequence) FROM ledger_cache WHERE expires_at > ?1",
                params![now],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        Ok(result.map(|v| v as u32))
    }

    /// Clean up expired cache entries and their events.
    pub fn cleanup_expired(&self) -> Result<u64, Error> {
        let now = chrono::Utc::now().timestamp();
        let expired: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT ledger_sequence FROM ledger_cache WHERE expires_at <= ?1")?;
            let rows = stmt.query_map(params![now], |row| row.get(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        if expired.is_empty() {
            return Ok(0);
        }

        let tx = self.conn.unchecked_transaction()?;
        let mut deleted = 0u64;
        for seq in &expired {
            tx.execute(
                "DELETE FROM events WHERE ledger_sequence = ?1",
                params![seq],
            )?;
            tx.execute(
                "DELETE FROM ledger_cache WHERE ledger_sequence = ?1",
                params![seq],
            )?;
            deleted += 1;
        }
        tx.commit()?;

        Ok(deleted)
    }
}

/// A structured event filter. Multiple filters are OR'd together; conditions within a
/// single filter are AND'd. Topics support positional matching with `"*"` as a wildcard.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct EventFilter {
    /// Filter by contract ID (Stellar strkey, e.g. "C...").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    /// Filter by event type: "contract", "system", or "diagnostic".
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    /// Positional topic matching. Each element is an XDR-JSON ScVal or the string `"*"`
    /// (wildcard). The filter matches if the event has at least as many topics and each
    /// non-wildcard position matches exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topics: Option<Vec<serde_json::Value>>,
}

/// Parameters for querying events.
#[derive(Debug, Default, Clone)]
pub struct EventQueryParams {
    pub limit: u32,
    pub after: Option<String>,
    pub ledger: Option<u32>,
    /// Filter all results to a single transaction hash.
    pub tx_hash: Option<String>,
    /// Structured filters. Each filter is OR'd; conditions within are AND'd.
    pub filters: Vec<EventFilter>,
}

/// Result of an event query.
#[derive(Debug)]
pub struct EventQueryResult {
    pub data: Vec<EventRow>,
    pub has_more: bool,
}

/// A single event row from the database.
#[derive(Debug, Clone)]
pub struct EventRow {
    pub id: String,
    pub ledger_sequence: u32,
    pub tx_index: u32,
    pub event_index: u32,
    pub ledger_closed_at: i64,
    pub contract_id: Option<String>,
    pub event_type: String,
    pub topics_json: String,
    pub data_json: String,
    pub event_json: String,
    pub tx_hash: String,
}
