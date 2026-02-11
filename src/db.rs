use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use dashmap::DashMap;

use crate::ledger::events::ExtractedEvent;

/// In-memory event store, partitioned by ledger sequence.
///
/// Each ledger's events are stored in an immutable partition behind an `Arc`,
/// enabling lock-free concurrent reads. Expired partitions are simply dropped
/// (O(1) cleanup vs. SQLite's expensive DELETE + VACUUM).
pub struct EventStore {
    /// Ledger sequence -> immutable partition.
    ledgers: DashMap<u32, Arc<LedgerPartition>>,
    /// Highest ledger sequence currently stored.
    latest_ledger: AtomicU32,
    /// Simple key-value store for sync state.
    sync_state: DashMap<String, String>,
    /// Cache TTL in seconds.
    cache_ttl_seconds: i64,
}

/// An immutable partition holding all events for a single ledger.
/// Built once during ingestion, never modified afterward.
struct LedgerPartition {
    /// Events sorted by ID for cursor-based pagination.
    events: Vec<StoredEvent>,
    /// Unix timestamp when this partition expires.
    expires_at: i64,
}

/// Internal event representation optimised for in-memory filtering.
struct StoredEvent {
    id: String,
    external_id: String,
    ledger_sequence: u32,
    ledger_closed_at: String,
    contract_id: Option<String>,
    /// 0 = contract, 1 = system, 2 = diagnostic
    event_type: u8,
    event_type_str: &'static str,
    topics: serde_json::Value,
    data: serde_json::Value,
    tx_hash: String,
}

impl StoredEvent {
    fn to_event_row(&self) -> EventRow {
        EventRow {
            id: self.external_id.clone(),
            ledger_sequence: self.ledger_sequence,
            ledger_closed_at: self.ledger_closed_at.clone(),
            contract_id: self.contract_id.clone(),
            event_type: self.event_type_str,
            topics: self.topics.clone(),
            data: self.data.clone(),
            tx_hash: self.tx_hash.clone(),
        }
    }

    /// Check whether this event matches a single filter (all conditions AND'd).
    fn matches_filter(&self, filter: &EventFilter) -> bool {
        if let Some(ref cid) = filter.contract_id {
            match &self.contract_id {
                Some(eid) if eid == cid => {}
                _ => return false,
            }
        }

        if let Some(ref et) = filter.event_type {
            let code = match et.as_str() {
                "contract" => 0u8,
                "system" => 1,
                "diagnostic" => 2,
                _ => return false,
            };
            if self.event_type != code {
                return false;
            }
        }

        if let Some(ref topics) = filter.topics {
            if !topics.is_empty() {
                let stored = match self.topics.as_array() {
                    Some(v) => v,
                    None => return false,
                };
                if stored.len() < topics.len() {
                    return false;
                }
                for (i, topic_val) in topics.iter().enumerate() {
                    if topic_val.is_null() {
                        continue;
                    }
                    match stored.get(i) {
                        Some(actual) if actual == topic_val => {}
                        _ => return false,
                    }
                }
            }
        }

        if let Some(ref any_topics) = filter.any_topics {
            let stored = match self.topics.as_array() {
                Some(v) => v,
                None => return false,
            };
            for required in any_topics {
                if !stored.iter().any(|actual| actual == required) {
                    return false;
                }
            }
        }

        true
    }
}

impl EventStore {
    /// Create a new in-memory event store.
    pub fn new(cache_ttl_seconds: i64) -> Self {
        Self {
            ledgers: DashMap::new(),
            latest_ledger: AtomicU32::new(0),
            sync_state: DashMap::new(),
            cache_ttl_seconds,
        }
    }

    /// Insert extracted events into the store, grouped by ledger.
    #[tracing::instrument(skip_all, fields(event_count = events.len()))]
    pub fn insert_events(&self, events: Vec<ExtractedEvent>) -> Result<(), crate::Error> {
        // Group events by ledger sequence.
        let mut by_ledger: HashMap<u32, Vec<ExtractedEvent>> = HashMap::new();
        for event in events {
            by_ledger
                .entry(event.ledger_sequence)
                .or_default()
                .push(event);
        }

        for (ledger_seq, ledger_events) in by_ledger {
            // Skip if already cached (idempotent).
            if self.ledgers.contains_key(&ledger_seq) {
                continue;
            }

            let mut stored: Vec<StoredEvent> = Vec::with_capacity(ledger_events.len());

            for event in ledger_events {
                let id = crate::ledger::event_id::event_id(
                    event.ledger_sequence,
                    event.phase,
                    event.tx_index,
                    event.event_index,
                );
                let (phase, sub) = event.phase.as_phase_sub();
                let external_id = crate::ledger::event_id::encode_event_id(
                    event.ledger_sequence,
                    phase,
                    event.tx_index,
                    sub,
                    event.event_index,
                );
                let (event_type, event_type_str) = match event.event_type {
                    crate::ledger::events::EventType::Contract => (0u8, "contract"),
                    crate::ledger::events::EventType::System => (1u8, "system"),
                    crate::ledger::events::EventType::Diagnostic => (2u8, "diagnostic"),
                };
                let ledger_closed_at = chrono::DateTime::from_timestamp(event.ledger_closed_at, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default();
                let topics = serde_json::Value::Array(event.topics_xdr_json);
                let data = event.data_xdr_json;

                stored.push(StoredEvent {
                    id,
                    external_id,
                    ledger_sequence: event.ledger_sequence,
                    ledger_closed_at,
                    contract_id: event.contract_id,
                    event_type,
                    event_type_str,
                    topics,
                    data,
                    tx_hash: event.tx_hash,
                });
            }

            // Sort by ID for cursor-based pagination.
            stored.sort_by(|a, b| a.id.cmp(&b.id));

            let now = chrono::Utc::now().timestamp();
            let partition = Arc::new(LedgerPartition {
                events: stored,
                expires_at: now + self.cache_ttl_seconds,
            });

            let event_count = partition.events.len();
            self.ledgers.insert(ledger_seq, partition);

            metrics::gauge!("store_partitions_total").set(self.ledgers.len() as f64);
            metrics::counter!("store_events_ingested_total").increment(event_count as u64);

            tracing::debug!(
                ledger = ledger_seq,
                events = event_count,
                "inserted ledger partition"
            );

            // Update latest ledger tracker.
            self.latest_ledger.fetch_max(ledger_seq, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Record that a ledger has been cached (sets TTL).
    pub fn record_ledger_cached(
        &self,
        ledger_sequence: u32,
        _ttl_seconds: i64,
    ) -> Result<(), crate::Error> {
        // In the in-memory store, TTL is set during insert_events.
        // This method exists for API compatibility. If the ledger was inserted
        // without events (empty ledger), record it now.
        if !self.ledgers.contains_key(&ledger_sequence) {
            let now = chrono::Utc::now().timestamp();
            let partition = Arc::new(LedgerPartition {
                events: Vec::new(),
                expires_at: now + self.cache_ttl_seconds,
            });
            self.ledgers.insert(ledger_sequence, partition);
            self.latest_ledger
                .fetch_max(ledger_sequence, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Check if a ledger is cached and not expired.
    pub fn is_ledger_cached(&self, ledger_sequence: u32) -> Result<bool, crate::Error> {
        let now = chrono::Utc::now().timestamp();
        Ok(self
            .ledgers
            .get(&ledger_sequence)
            .is_some_and(|p| p.expires_at > now))
    }

    /// Find ledger sequences in the given range that are NOT cached.
    pub fn find_uncached_ledgers(&self, start: u32, count: u32) -> Result<Vec<u32>, crate::Error> {
        let now = chrono::Utc::now().timestamp();
        let end = start + count;
        Ok((start..end)
            .filter(|seq| self.ledgers.get(seq).is_none_or(|p| p.expires_at <= now))
            .collect())
    }

    /// Get sync state value.
    pub fn get_sync_state(&self, key: &str) -> Result<Option<String>, crate::Error> {
        Ok(self.sync_state.get(key).map(|v| v.value().clone()))
    }

    /// Set sync state value.
    pub fn set_sync_state(&self, key: &str, value: &str) -> Result<(), crate::Error> {
        self.sync_state.insert(key.to_string(), value.to_string());
        Ok(())
    }

    /// Query events with cursor-based pagination and filtering.
    ///
    /// Results are always returned in descending order (newest first).
    /// - `after` cursor: selects events newer than the cursor, returned desc.
    /// - `before` cursor: selects events older than the cursor, returned desc.
    /// - No cursor: returns the most recent events.
    #[tracing::instrument(skip_all, fields(limit = params.limit, ledger = params.ledger, filters = params.filters.len()))]
    pub fn query_events(
        &self,
        params: &EventQueryParams,
    ) -> Result<EventQueryResult, crate::Error> {
        let cursor = params.after.as_ref().or(params.before.as_ref());

        // Determine the pinned ledger.
        let pinned_ledger = match (cursor, params.ledger) {
            (None, None) => {
                // Auto-select the latest ledger.
                let latest = self.latest_ledger.load(Ordering::Relaxed);
                if latest == 0 {
                    None
                } else {
                    Some(latest)
                }
            }
            _ => params.ledger,
        };

        // If we have a pinned ledger, query that single partition.
        if let Some(seq) = pinned_ledger {
            return self.query_single_ledger(seq, params);
        }

        // Cross-ledger query (cursor provided, no ledger pin).
        if let Some(ref after) = params.after {
            return self.query_cross_ledger_after(after, params);
        }
        if let Some(ref before) = params.before {
            return self.query_cross_ledger_before(before, params);
        }

        // No ledger, no cursor — return empty.
        Ok(EventQueryResult {
            data: Vec::new(),
            next: None,
        })
    }

    /// Query events within a single ledger partition.
    ///
    /// Always returns results in descending order (newest first).
    fn query_single_ledger(
        &self,
        ledger_seq: u32,
        params: &EventQueryParams,
    ) -> Result<EventQueryResult, crate::Error> {
        let partition = match self.ledgers.get(&ledger_seq) {
            Some(p) => Arc::clone(p.value()),
            None => {
                return Ok(EventQueryResult {
                    data: Vec::new(),
                    next: None,
                })
            }
        };

        let events = &partition.events;
        let limit = params.limit as usize;

        if let Some(ref after) = params.after {
            // `after` cursor: select events with id > after, iterate forward,
            // then reverse for descending display. The `next` cursor advances
            // forward so the client can pass it as `after` again.
            let start = match events.binary_search_by(|e| e.id.as_str().cmp(after.as_str())) {
                Ok(pos) => pos + 1,
                Err(pos) => pos,
            };

            let mut results: Vec<EventRow> = Vec::with_capacity(limit.min(events.len()));
            let mut last_examined_id: Option<&str> = None;

            for event in events.iter().skip(start) {
                if results.len() >= limit {
                    break;
                }
                last_examined_id = Some(&event.external_id);
                if !self.event_matches(event, params) {
                    continue;
                }
                results.push(event.to_event_row());
            }

            results.reverse();
            Ok(EventQueryResult {
                data: results,
                next: last_examined_id.map(|id| id.to_owned()),
            })
        } else {
            // No cursor or `before` cursor: iterate backward (already desc).
            let end = match &params.before {
                Some(before) => {
                    match events.binary_search_by(|e| e.id.as_str().cmp(before.as_str())) {
                        Ok(pos) => pos,
                        Err(pos) => pos,
                    }
                }
                None => events.len(),
            };

            let mut results: Vec<EventRow> = Vec::with_capacity(limit.min(events.len()));
            let mut last_examined_id: Option<&str> = None;

            for event in events[..end].iter().rev() {
                if results.len() >= limit {
                    break;
                }
                last_examined_id = Some(&event.external_id);
                if !self.event_matches(event, params) {
                    continue;
                }
                results.push(event.to_event_row());
            }

            Ok(EventQueryResult {
                data: results,
                next: last_examined_id.map(|id| id.to_owned()),
            })
        }
    }

    /// Check whether a single event passes the tx and filter constraints.
    fn event_matches(&self, event: &StoredEvent, params: &EventQueryParams) -> bool {
        if let Some(ref tx) = params.tx {
            if event.tx_hash != *tx {
                return false;
            }
        }
        if !params.filters.is_empty() && !params.filters.iter().any(|f| event.matches_filter(f)) {
            return false;
        }
        true
    }

    /// Cross-ledger query with `after` cursor: iterate forward across ledgers,
    /// then reverse the collected results for descending display.
    fn query_cross_ledger_after(
        &self,
        after: &str,
        params: &EventQueryParams,
    ) -> Result<EventQueryResult, crate::Error> {
        let mut ledger_seqs: Vec<u32> = self.ledgers.iter().map(|kv| *kv.key()).collect();
        ledger_seqs.sort_unstable();

        let cursor_ledger =
            crate::ledger::event_id::parse_event_id(after).map(|(seq, _, _, _, _)| seq);

        let limit = params.limit as usize;
        let mut results: Vec<EventRow> = Vec::with_capacity(limit);
        let mut last_examined_id: Option<String> = None;

        for &seq in &ledger_seqs {
            if let Some(cl) = cursor_ledger {
                if seq < cl {
                    continue;
                }
            }

            if results.len() >= limit {
                break;
            }

            let partition = match self.ledgers.get(&seq) {
                Some(p) => Arc::clone(p.value()),
                None => continue,
            };

            let events = &partition.events;
            let start = if Some(seq) == cursor_ledger {
                match events.binary_search_by(|e| e.id.as_str().cmp(after)) {
                    Ok(pos) => pos + 1,
                    Err(pos) => pos,
                }
            } else {
                0
            };

            for event in events.iter().skip(start) {
                if results.len() >= limit {
                    break;
                }
                last_examined_id = Some(event.external_id.clone());
                if !self.event_matches(event, params) {
                    continue;
                }
                results.push(event.to_event_row());
            }
        }

        results.reverse();
        Ok(EventQueryResult {
            data: results,
            next: last_examined_id,
        })
    }

    /// Cross-ledger query with `before` cursor: iterate backward across ledgers
    /// (already descending).
    fn query_cross_ledger_before(
        &self,
        before: &str,
        params: &EventQueryParams,
    ) -> Result<EventQueryResult, crate::Error> {
        let mut ledger_seqs: Vec<u32> = self.ledgers.iter().map(|kv| *kv.key()).collect();
        ledger_seqs.sort_unstable_by(|a, b| b.cmp(a));

        let cursor_ledger =
            crate::ledger::event_id::parse_event_id(before).map(|(seq, _, _, _, _)| seq);

        let limit = params.limit as usize;
        let mut results: Vec<EventRow> = Vec::with_capacity(limit);
        let mut last_examined_id: Option<String> = None;

        for &seq in &ledger_seqs {
            if let Some(cl) = cursor_ledger {
                if seq > cl {
                    continue;
                }
            }

            if results.len() >= limit {
                break;
            }

            let partition = match self.ledgers.get(&seq) {
                Some(p) => Arc::clone(p.value()),
                None => continue,
            };

            let events = &partition.events;
            let end = if Some(seq) == cursor_ledger {
                match events.binary_search_by(|e| e.id.as_str().cmp(before)) {
                    Ok(pos) => pos,
                    Err(pos) => pos,
                }
            } else {
                events.len()
            };

            for event in events[..end].iter().rev() {
                if results.len() >= limit {
                    break;
                }
                last_examined_id = Some(event.external_id.clone());
                if !self.event_matches(event, params) {
                    continue;
                }
                results.push(event.to_event_row());
            }
        }

        Ok(EventQueryResult {
            data: results,
            next: last_examined_id,
        })
    }

    /// Get the highest ledger sequence in the store.
    pub fn latest_ledger_sequence(&self) -> Result<Option<u32>, crate::Error> {
        let v = self.latest_ledger.load(Ordering::Relaxed);
        Ok(if v == 0 { None } else { Some(v) })
    }

    /// Get the number of ledgers currently cached.
    pub fn cached_ledger_count(&self) -> usize {
        self.ledgers.len()
    }

    /// Clean up expired cache entries. Returns the number of ledgers removed.
    #[tracing::instrument(skip_all)]
    pub fn cleanup_expired(&self) -> Result<u64, crate::Error> {
        let now = chrono::Utc::now().timestamp();
        let mut removed = 0u64;

        // Collect expired keys first to avoid holding iterators during removal.
        let expired: Vec<u32> = self
            .ledgers
            .iter()
            .filter(|kv| kv.value().expires_at <= now)
            .map(|kv| *kv.key())
            .collect();

        for seq in expired {
            self.ledgers.remove(&seq);
            removed += 1;
        }

        // Update latest_ledger if the current one was removed.
        if removed > 0 {
            let new_latest = self.ledgers.iter().map(|kv| *kv.key()).max().unwrap_or(0);
            self.latest_ledger.store(new_latest, Ordering::Relaxed);
            metrics::gauge!("store_partitions_total").set(self.ledgers.len() as f64);
            metrics::counter!("store_partitions_expired_total").increment(removed);
            tracing::debug!(
                removed,
                remaining = self.ledgers.len(),
                "expired partitions removed"
            );
        }

        Ok(removed)
    }

    /// Look up a single event by ledger sequence and internal ID.
    pub fn get_event(
        &self,
        ledger_seq: u32,
        internal_id: &str,
    ) -> Result<Option<EventRow>, crate::Error> {
        let partition = match self.ledgers.get(&ledger_seq) {
            Some(p) => Arc::clone(p.value()),
            None => return Ok(None),
        };

        match partition
            .events
            .binary_search_by(|e| e.id.as_str().cmp(internal_id))
        {
            Ok(pos) => Ok(Some(partition.events[pos].to_event_row())),
            Err(_) => Ok(None),
        }
    }

    /// No-op (no query planner in in-memory store).
    pub fn analyze(&self) -> Result<(), crate::Error> {
        Ok(())
    }
}

/// A structured event filter. Multiple filters are OR'd together; conditions within a
/// single filter are AND'd. Topics support positional matching with `null` as a wildcard.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct EventFilter {
    /// Filter by contract ID (Stellar strkey, e.g. "C...").
    #[serde(rename = "contract", default, skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    /// Filter by event type: "contract" or "system".
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    /// Positional topic matching. Each element is an XDR-JSON ScVal or `null`
    /// (wildcard). The filter matches if the event has at least as many topics and each
    /// non-wildcard position matches exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topics: Option<Vec<serde_json::Value>>,
    /// Non-positional topic matching. Each element is an XDR-JSON ScVal that must
    /// appear in at least one topic position. Multiple values are AND'd (all must match).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub any_topics: Option<Vec<serde_json::Value>>,
}

/// Parameters for querying events.
#[derive(Debug, Default, Clone)]
pub struct EventQueryParams {
    pub limit: u32,
    pub after: Option<String>,
    pub before: Option<String>,
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
    /// Cursor for the next page. Pass as `before` to continue paginating backward,
    /// or as `after` to continue polling forward. When filters are applied, this may
    /// point beyond the last returned event to avoid re-scanning examined ranges.
    pub next: Option<String>,
}

/// A single event row returned from queries.
#[derive(Debug, Clone)]
pub struct EventRow {
    pub id: String,
    pub ledger_sequence: u32,
    pub ledger_closed_at: String,
    pub contract_id: Option<String>,
    pub event_type: &'static str,
    pub topics: serde_json::Value,
    pub data: serde_json::Value,
    pub tx_hash: String,
}
