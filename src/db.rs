use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::ledger::events::ExtractedEvent;
use crate::Error;

/// SQLite-backed event store.
pub struct EventStore {
    conn: Connection,
}

/// Apply performance-oriented PRAGMAs to the connection.
fn configure_pragmas(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA cache_size = -64000;
        PRAGMA mmap_size = 268435456;
        PRAGMA temp_store = MEMORY;
        PRAGMA page_size = 8192;
        PRAGMA auto_vacuum = INCREMENTAL;
        ",
    )?;
    Ok(())
}

impl EventStore {
    /// Open or create the database at the given path.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let conn = Connection::open(path)?;
        configure_pragmas(&conn)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Open a read-only connection (for query serving).
    pub fn open_readonly(path: &Path) -> Result<Self, Error> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        configure_pragmas(&conn)?;
        let store = Self { conn };
        Ok(store)
    }

    /// Open an in-memory database (for testing).
    pub fn open_memory() -> Result<Self, Error> {
        let conn = Connection::open_in_memory()?;
        configure_pragmas(&conn)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), Error> {
        // Check if we need to migrate from the old schema
        let has_event_json: bool = self
            .conn
            .prepare("SELECT 1 FROM pragma_table_info('events') WHERE name='event_json'")?
            .exists([])?;

        if has_event_json {
            // Old schema detected — drop and recreate
            self.conn.execute_batch("DROP TABLE IF EXISTS events;")?;
        }

        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS events (
                id TEXT NOT NULL,
                ledger_sequence INTEGER NOT NULL,
                ledger_closed_at INTEGER NOT NULL,
                contract_id TEXT,
                event_type INTEGER NOT NULL DEFAULT 0,
                topic0 TEXT,
                topic_count INTEGER NOT NULL DEFAULT 0,
                topics_json TEXT NOT NULL,
                data_json TEXT NOT NULL,
                tx_hash TEXT NOT NULL
            );

            -- Primary lookup: ledger + id ordering (covers ledger= queries with ORDER BY id)
            CREATE UNIQUE INDEX IF NOT EXISTS idx_events_pk ON events(id);
            CREATE INDEX IF NOT EXISTS idx_events_ledger_id ON events(ledger_sequence, id);

            -- Contract filter: narrows within a ledger, includes id for ordering
            CREATE INDEX IF NOT EXISTS idx_events_contract_ledger ON events(contract_id, ledger_sequence, id);

            -- Topic0 filter: fast first-topic lookup
            CREATE INDEX IF NOT EXISTS idx_events_topic0_ledger ON events(topic0, ledger_sequence, id);

            -- tx_hash filter
            CREATE INDEX IF NOT EXISTS idx_events_tx ON events(tx_hash, id);

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

        // Drop legacy indexes that are now superseded
        self.conn.execute_batch(
            "
            DROP INDEX IF EXISTS idx_events_ledger;
            DROP INDEX IF EXISTS idx_events_contract;
            DROP INDEX IF EXISTS idx_events_type;
            DROP INDEX IF EXISTS idx_events_id;
            ",
        )?;

        Ok(())
    }

    /// Insert extracted events into the database.
    pub fn insert_events(&self, events: &[ExtractedEvent]) -> Result<(), Error> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO events (id, ledger_sequence, ledger_closed_at, contract_id, event_type, topic0, topic_count, topics_json, data_json, tx_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
                // Extract topic0 as a canonical JSON string for indexed lookups
                let topic0: Option<String> = event
                    .topics_xdr_json
                    .first()
                    .map(|v| serde_json::to_string(v).unwrap_or_default());
                let topic_count = event.topics_xdr_json.len() as i32;
                let event_type_int = match event.event_type {
                    crate::ledger::events::EventType::Contract => 0i32,
                    crate::ledger::events::EventType::System => 1i32,
                    crate::ledger::events::EventType::Diagnostic => 2i32,
                };
                stmt.execute(params![
                    id,
                    event.ledger_sequence,
                    event.ledger_closed_at,
                    event.contract_id,
                    event_type_int,
                    topic0,
                    topic_count,
                    topics_json,
                    data_json,
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

    /// Find ledger sequences in the given range that are NOT cached (single query).
    pub fn find_uncached_ledgers(&self, start: u32, count: u32) -> Result<Vec<u32>, Error> {
        let now = chrono::Utc::now().timestamp();
        let end = start + count;
        // Get the set of cached ledgers in one query
        let mut stmt = self.conn.prepare_cached(
            "SELECT ledger_sequence FROM ledger_cache WHERE ledger_sequence >= ?1 AND ledger_sequence < ?2 AND expires_at > ?3",
        )?;
        let cached: std::collections::HashSet<u32> = stmt
            .query_map(params![start, end, now], |row| {
                row.get::<_, i64>(0).map(|v| v as u32)
            })?
            .collect::<Result<_, _>>()?;

        Ok((start..end).filter(|seq| !cached.contains(seq)).collect())
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
        let pinned_ledger = match (params.after.as_ref(), params.ledger) {
            (None, None) => {
                // Use ledger_cache (small PK-indexed table) for fast MAX lookup
                let max: Option<i64> = self
                    .conn
                    .query_row("SELECT MAX(ledger_sequence) FROM ledger_cache", [], |row| {
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

        // Transaction hash filter
        if let Some(ref tx_hash) = params.tx {
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
                    let et_int = match et.as_str() {
                        "contract" => 0i32,
                        "system" => 1i32,
                        "diagnostic" => 2i32,
                        _ => -1i32,
                    };
                    fc.push("event_type = ?".to_string());
                    bind_values.push(Box::new(et_int));
                }

                if let Some(ref topics) = filter.topics {
                    if !topics.is_empty() {
                        // Use the indexed topic_count column instead of json_array_length
                        fc.push(format!("topic_count >= {}", topics.len()));

                        // Use the indexed topic0 column for position 0 lookups
                        for (i, topic) in topics.iter().enumerate() {
                            if topic.as_str() == Some("*") {
                                continue;
                            }
                            let topic_json = serde_json::to_string(topic).unwrap_or_default();
                            if i == 0 {
                                // Use the dedicated indexed column
                                fc.push("topic0 = ?".to_string());
                            } else {
                                fc.push(format!("json_extract(topics_json, '$[{}]') = json(?)", i));
                            }
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
            "SELECT id, ledger_sequence, ledger_closed_at, contract_id, event_type, topics_json, data_json, tx_hash FROM events {} ORDER BY id ASC LIMIT ?",
            where_clause
        );

        bind_values.push(Box::new(fetch_limit as i64));

        let mut stmt = self.conn.prepare_cached(&sql)?;
        let bind_refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|v| v.as_ref()).collect();

        let rows = stmt.query_map(bind_refs.as_slice(), |row| {
            let event_type_int: i32 = row.get(4)?;
            let event_type_str = match event_type_int {
                0 => "contract",
                1 => "system",
                2 => "diagnostic",
                _ => "unknown",
            };
            Ok(EventRow {
                id: row.get(0)?,
                ledger_sequence: row.get::<_, i64>(1)? as u32,
                ledger_closed_at: row.get(2)?,
                contract_id: row.get(3)?,
                event_type: event_type_str.to_string(),
                topics_json: row.get(5)?,
                data_json: row.get(6)?,
                tx_hash: row.get(7)?,
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

        let tx = self.conn.unchecked_transaction()?;
        // Delete events whose ledger is expired in one batch
        tx.execute(
            "DELETE FROM events WHERE ledger_sequence IN (SELECT ledger_sequence FROM ledger_cache WHERE expires_at <= ?1)",
            params![now],
        )?;
        let deleted = tx.execute(
            "DELETE FROM ledger_cache WHERE expires_at <= ?1",
            params![now],
        )?;
        tx.commit()?;

        // Reclaim freed pages
        if deleted > 0 {
            let _ = self.conn.execute_batch("PRAGMA incremental_vacuum;");
        }

        Ok(deleted as u64)
    }

    /// Run PRAGMA optimize to update query planner statistics where needed.
    pub fn analyze(&self) -> Result<(), Error> {
        self.conn.execute_batch("PRAGMA optimize;")?;
        Ok(())
    }
}

/// A structured event filter. Multiple filters are OR'd together; conditions within a
/// single filter are AND'd. Topics support positional matching with `"*"` as a wildcard.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct EventFilter {
    /// Filter by contract ID (Stellar strkey, e.g. "C...").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    /// Filter by event type: "contract" or "system".
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
    pub tx: Option<String>,
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
    pub ledger_closed_at: i64,
    pub contract_id: Option<String>,
    pub event_type: String,
    pub topics_json: String,
    pub data_json: String,
    pub tx_hash: String,
}
