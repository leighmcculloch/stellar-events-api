use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;

use super::error::ApiError;
use super::types::{BuildInfo, Event, ListResponse, PrettyJson, StatusResponse};
use crate::db::{EventQueryParams, EventQueryResult, EventRow};
use crate::{sync, AppState};

/// Maximum number of ledgers to backfill per request.
const BACKFILL_BATCH_SIZE: u32 = 100;

/// Maximum number of ledgers to search during progressive backfill.
const MAX_LEDGERS_SEARCHED: u32 = 1000;

/// Time limit for progressive search across ledgers.
const PROGRESSIVE_SEARCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// GET /
pub async fn home() -> axum::response::Html<&'static str> {
    axum::response::Html(HOME_HTML)
}

const HOME_HTML: &str = include_str!("home.html");

/// Parse a raw query string into a multi-map (key -> Vec<value>).
/// Supports both `key=a&key=b` and `key[]=a&key[]=b` styles.
fn parse_multi_params(query: &str) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        let decoded_key = urlencoding::decode(key).unwrap_or_else(|_| key.into());
        let decoded_val = urlencoding::decode(value).unwrap_or_else(|_| value.into());
        // Normalize `key[]` to `key`
        let normalized_key = decoded_key.trim_end_matches("[]").to_string();
        map.entry(normalized_key)
            .or_default()
            .push(decoded_val.to_string());
    }
    map
}

/// JSON request body for POST /events.
#[derive(Debug, Default, serde::Deserialize)]
pub struct ListEventsRequest {
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    q: Option<String>,
}

/// GET /events
#[tracing::instrument(skip_all, fields(method = "GET"))]
pub async fn list_events_get(
    State(state): State<Arc<AppState>>,
    axum::extract::RawQuery(raw_query): axum::extract::RawQuery,
) -> Result<impl IntoResponse, ApiError> {
    let query_str = raw_query.unwrap_or_default();
    let multi = parse_multi_params(&query_str);

    let limit = match multi.get("limit").and_then(|v| v.first()) {
        Some(v) => Some(v.parse::<u32>().map_err(|_| ApiError::BadRequest {
            message: "limit must be a positive integer".to_string(),
            param: Some("limit".to_string()),
        })?),
        None => None,
    };

    let after = multi.get("after").and_then(|v| v.first()).cloned();
    let before = multi.get("before").and_then(|v| v.first()).cloned();
    let q = multi.get("q").and_then(|v| v.first()).cloned();

    let req = ListEventsRequest {
        limit,
        after,
        before,
        q,
    };

    list_events(state, req).await
}

/// POST /events
#[tracing::instrument(skip_all, fields(method = "POST"))]
pub async fn list_events_post(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListEventsRequest>,
) -> Result<impl IntoResponse, ApiError> {
    list_events(state, req).await
}

/// Fetch and cache a single ledger on demand, bypassing the latest-synced watermark.
#[tracing::instrument(skip(state))]
async fn backfill_ledger(state: &AppState, ledger_seq: u32) {
    if state
        .store
        .find_uncached_ledgers(ledger_seq, 1)
        .unwrap_or_default()
        .is_empty()
    {
        return;
    }

    match sync::fetch_and_extract(&state.client, &state.meta_url, &state.config, ledger_seq).await {
        Ok(events) => {
            if let Err(e) = state.store.insert_events(events) {
                tracing::warn!(ledger = ledger_seq, error = %e, "backfill_ledger: failed to insert events");
                return;
            }
            if let Err(e) = state.store.record_ledger_cached(ledger_seq, 0) {
                tracing::warn!(ledger = ledger_seq, error = %e, "backfill_ledger: failed to record cache");
            }
        }
        Err(crate::Error::LedgerNotFound(_)) => {}
        Err(e) => {
            tracing::warn!(ledger = ledger_seq, error = %e, "backfill_ledger: failed to fetch ledger");
        }
    }
}

struct BackfillResult {
    hit_not_found: bool,
}

/// Fetch and cache a batch of uncached ledgers concurrently from S3.
#[tracing::instrument(skip_all, fields(count = uncached.len()))]
async fn backfill_batch(state: &AppState, uncached: &[u32]) -> BackfillResult {
    tracing::debug!(count = uncached.len(), "backfilling uncached ledgers");

    let futures: Vec<_> = uncached
        .iter()
        .map(|&seq| sync::fetch_and_extract(&state.client, &state.meta_url, &state.config, seq))
        .collect();
    let results = futures::future::join_all(futures).await;

    let mut hit_not_found = false;
    for (i, result) in results.into_iter().enumerate() {
        let seq = uncached[i];
        match result {
            Ok(events) => {
                if let Err(e) = state.store.insert_events(events) {
                    tracing::warn!(ledger = seq, error = %e, "backfill: failed to insert events");
                    continue;
                }
                if let Err(e) = state.store.record_ledger_cached(seq, 0) {
                    tracing::warn!(ledger = seq, error = %e, "backfill: failed to record cache");
                }
            }
            Err(crate::Error::LedgerNotFound(_)) => {
                hit_not_found = true;
            }
            Err(e) => {
                tracing::warn!(ledger = seq, error = %e, "backfill: failed to fetch ledger");
            }
        }
    }

    BackfillResult { hit_not_found }
}

/// Fetch and cache historical ledgers on demand, starting at `target_ledger`.
#[tracing::instrument(skip(state))]
async fn backfill_if_needed(state: &AppState, target_ledger: u32) {
    let latest = state
        .store
        .latest_ledger_sequence()
        .ok()
        .flatten()
        .unwrap_or(0);
    if target_ledger > latest {
        return;
    }
    let range = BACKFILL_BATCH_SIZE.min(latest.saturating_sub(target_ledger) + 1);
    let uncached = state
        .store
        .find_uncached_ledgers(target_ledger, range)
        .unwrap_or_default();

    if uncached.is_empty() {
        return;
    }

    backfill_batch(state, &uncached).await;
}

/// Progressive backward query: iteratively fetch and scan ledgers from newest
/// to oldest until the limit is filled or a stopping condition is reached.
#[tracing::instrument(skip_all, fields(limit = params.limit))]
async fn query_progressive_backward(
    state: &AppState,
    params: &EventQueryParams,
) -> Result<EventQueryResult, crate::Error> {
    let latest = state.store.latest_ledger_sequence()?.unwrap_or(0);
    let start_ledger = if let Some(ref before) = params.before {
        crate::ledger::event_id::parse_event_id(before)
            .map(|(seq, _, _, _, _)| seq)
            .unwrap_or(latest)
    } else {
        latest
    };

    if start_ledger == 0 {
        return Ok(EventQueryResult {
            data: Vec::new(),
            next: None,
        });
    }

    let limit = params.limit as usize;
    let mut results: Vec<EventRow> = Vec::with_capacity(limit);
    let mut last_examined_id: Option<String> = None;
    let mut ledgers_searched: u32 = 0;
    let mut current = start_ledger;
    let deadline = std::time::Instant::now() + PROGRESSIVE_SEARCH_TIMEOUT;

    loop {
        if results.len() >= limit
            || ledgers_searched >= MAX_LEDGERS_SEARCHED
            || std::time::Instant::now() >= deadline
        {
            break;
        }

        let batch_size = BACKFILL_BATCH_SIZE.min(current + 1);
        let batch_start = current + 1 - batch_size;

        let uncached = state.store.find_uncached_ledgers(batch_start, batch_size)?;
        let hit_not_found = if !uncached.is_empty() {
            backfill_batch(state, &uncached).await.hit_not_found
        } else {
            false
        };

        for seq in (batch_start..=current).rev() {
            if results.len() >= limit
                || ledgers_searched >= MAX_LEDGERS_SEARCHED
                || std::time::Instant::now() >= deadline
            {
                break;
            }
            ledgers_searched += 1;

            let remaining = limit - results.len();
            let cursor = if seq == start_ledger {
                params.before.as_deref()
            } else {
                None
            };
            if let Some(id) =
                state
                    .store
                    .scan_ledger_backward(seq, cursor, params, &mut results, remaining)
            {
                last_examined_id = Some(id);
            }
        }

        if hit_not_found || batch_start == 0 {
            break;
        }
        current = batch_start - 1;
    }

    Ok(EventQueryResult {
        data: results,
        next: last_examined_id,
    })
}

/// Progressive forward query: iteratively fetch and scan ledgers from the
/// cursor position toward the latest ledger, then reverse for descending output.
#[tracing::instrument(skip_all, fields(limit = params.limit))]
async fn query_progressive_forward(
    state: &AppState,
    params: &EventQueryParams,
) -> Result<EventQueryResult, crate::Error> {
    let after = params
        .after
        .as_deref()
        .ok_or_else(|| crate::Error::Internal("forward query requires after cursor".to_string()))?;
    let latest = state.store.latest_ledger_sequence()?.unwrap_or(0);

    let start_ledger = crate::ledger::event_id::parse_event_id(after)
        .map(|(seq, _, _, _, _)| seq)
        .unwrap_or(0);

    if start_ledger == 0 || latest == 0 {
        return Ok(EventQueryResult {
            data: Vec::new(),
            next: None,
        });
    }

    let limit = params.limit as usize;
    let mut results: Vec<EventRow> = Vec::with_capacity(limit);
    let mut last_examined_id: Option<String> = None;
    let mut ledgers_searched: u32 = 0;
    let mut current = start_ledger;
    let deadline = std::time::Instant::now() + PROGRESSIVE_SEARCH_TIMEOUT;

    loop {
        if results.len() >= limit
            || current > latest
            || ledgers_searched >= MAX_LEDGERS_SEARCHED
            || std::time::Instant::now() >= deadline
        {
            break;
        }

        let batch_size = BACKFILL_BATCH_SIZE.min(latest - current + 1);

        let uncached = state.store.find_uncached_ledgers(current, batch_size)?;
        let hit_not_found = if !uncached.is_empty() {
            backfill_batch(state, &uncached).await.hit_not_found
        } else {
            false
        };

        let batch_end = current + batch_size - 1;
        for seq in current..=batch_end {
            if results.len() >= limit
                || ledgers_searched >= MAX_LEDGERS_SEARCHED
                || std::time::Instant::now() >= deadline
            {
                break;
            }
            ledgers_searched += 1;

            let remaining = limit - results.len();
            let cursor = if seq == start_ledger {
                Some(after)
            } else {
                None
            };
            if let Some(id) =
                state
                    .store
                    .scan_ledger_forward(seq, cursor, params, &mut results, remaining)
            {
                last_examined_id = Some(id);
            }
        }

        if hit_not_found || batch_end >= latest {
            break;
        }
        current = batch_end + 1;
    }

    results.reverse();

    Ok(EventQueryResult {
        data: results,
        next: last_examined_id,
    })
}

#[tracing::instrument(skip_all, fields(limit = req.limit))]
async fn list_events(
    state: Arc<AppState>,
    req: ListEventsRequest,
) -> Result<PrettyJson<ListResponse<Event>>, ApiError> {
    let start = std::time::Instant::now();
    let limit = req.limit.unwrap_or(10);

    if limit == 0 || limit > 100 {
        return Err(ApiError::BadRequest {
            message: "limit must be between 1 and 100".to_string(),
            param: Some("limit".to_string()),
        });
    }

    // Validate mutual exclusivity of after and before.
    if req.after.is_some() && req.before.is_some() {
        return Err(ApiError::BadRequest {
            message: "after and before cannot both be provided".to_string(),
            param: Some("before".to_string()),
        });
    }

    // Validate and convert cursor from external to internal format if provided.
    let after = if let Some(ref cursor) = req.after {
        // Try decoding as an external (opaque) ID first, fall back to internal format.
        if let Some(internal) = crate::ledger::event_id::to_internal_id(cursor) {
            Some(internal)
        } else if crate::ledger::event_id::parse_event_id(cursor).is_some() {
            Some(cursor.clone())
        } else {
            return Err(ApiError::BadRequest {
                message: format!("invalid cursor: {}", cursor),
                param: Some("after".to_string()),
            });
        }
    } else {
        None
    };

    let before = if let Some(ref cursor) = req.before {
        if let Some(internal) = crate::ledger::event_id::to_internal_id(cursor) {
            Some(internal)
        } else if crate::ledger::event_id::parse_event_id(cursor).is_some() {
            Some(cursor.clone())
        } else {
            return Err(ApiError::BadRequest {
                message: format!("invalid cursor: {}", cursor),
                param: Some("before".to_string()),
            });
        }
    } else {
        None
    };

    // Parse q parameter into filters.
    let filters = match req.q.as_ref() {
        Some(q_str) => {
            super::query_parser::parse_query(q_str).map_err(|e| ApiError::BadRequest {
                message: format!("invalid q parameter: {}", e.message),
                param: Some("q".to_string()),
            })?
        }
        None => Vec::new(),
    };

    let filter_ledger = filters.iter().find_map(|f| f.ledger);

    let params = EventQueryParams {
        limit,
        after,
        before,
        filters,
    };

    let result = if let Some(target) = filter_ledger {
        // Ledger-pinned query: backfill the target range and query that partition.
        backfill_if_needed(&state, target).await;
        state
            .store
            .query_single_ledger(target, &params)
            .map_err(|e| ApiError::Internal {
                message: format!("database error: {}", e),
            })?
    } else if params.after.is_some() {
        // Progressive forward from cursor toward latest ledger.
        query_progressive_forward(&state, &params)
            .await
            .map_err(|e| ApiError::Internal {
                message: format!("database error: {}", e),
            })?
    } else {
        // Progressive backward from latest (or before cursor) toward oldest.
        query_progressive_backward(&state, &params)
            .await
            .map_err(|e| ApiError::Internal {
                message: format!("database error: {}", e),
            })?
    };

    tracing::debug!(events = result.data.len(), "query complete");

    let events: Vec<Event> = result.data.into_iter().map(Event::from).collect();

    metrics::counter!("api_requests_total", "endpoint" => "events").increment(1);
    metrics::histogram!("api_request_duration_seconds", "endpoint" => "events")
        .record(start.elapsed().as_secs_f64());
    metrics::histogram!("api_events_returned").record(events.len() as f64);

    let response = ListResponse {
        object: "list",
        url: "/events".to_string(),
        next: result.next,
        data: events,
    };

    Ok(PrettyJson(response))
}

/// GET /health
#[tracing::instrument(skip_all)]
pub async fn health(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, ApiError> {
    let latest = state
        .store
        .latest_ledger_sequence()
        .map_err(|e| ApiError::Internal {
            message: format!("database error: {}", e),
        })?;

    let response = StatusResponse {
        status: "ok".to_string(),
        latest_ledger: latest,
        cached_ledgers: state.store.cached_ledger_count(),
        network_passphrase: state.config.network_passphrase.clone(),
        build: BuildInfo {
            repo: option_env!("BUILD_REPO").unwrap_or(""),
            branch: option_env!("BUILD_BRANCH").unwrap_or(""),
            commit: option_env!("BUILD_COMMIT").unwrap_or(""),
            pr: option_env!("BUILD_PR").unwrap_or(""),
        },
    };

    Ok(PrettyJson(response))
}

/// GET /events/:id
#[tracing::instrument(skip_all, fields(id = %id))]
pub async fn get_event(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let start = std::time::Instant::now();

    // Decode the external ID to get components.
    let (ledger_seq, phase, tx_index, sub, event_index) =
        crate::ledger::event_id::decode_event_id(&id).ok_or_else(|| ApiError::NotFound {
            message: format!("event not found: {}", id),
        })?;

    // Reconstruct EventPhase; return 404 for invalid (phase, sub) combinations.
    let event_phase = match (phase, sub) {
        (0, 0) => crate::ledger::event_id::EventPhase::BeforeAllTxs,
        (1, 0) => crate::ledger::event_id::EventPhase::Operation,
        (1, 1) => crate::ledger::event_id::EventPhase::AfterTx,
        (2, 0) => crate::ledger::event_id::EventPhase::AfterAllTxs,
        _ => {
            return Err(ApiError::NotFound {
                message: format!("event not found: {}", id),
            })
        }
    };

    // Reconstruct internal ID for store lookup.
    let internal_id =
        crate::ledger::event_id::event_id(ledger_seq, event_phase, tx_index, event_index);

    // Backfill the ledger on demand. Use direct fetch since the event was
    // requested by ID — don't skip based on the latest-synced watermark.
    backfill_ledger(&state, ledger_seq).await;

    let row = state
        .store
        .get_event(ledger_seq, &internal_id)
        .map_err(|e| ApiError::Internal {
            message: format!("database error: {}", e),
        })?
        .ok_or_else(|| ApiError::NotFound {
            message: format!("event not found: {}", id),
        })?;

    let event = Event::from(row);

    metrics::counter!("api_requests_total", "endpoint" => "get_event").increment(1);
    metrics::histogram!("api_request_duration_seconds", "endpoint" => "get_event")
        .record(start.elapsed().as_secs_f64());

    Ok(PrettyJson(event))
}
