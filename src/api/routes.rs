use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;

use super::error::ApiError;
use super::types::{Event, ListResponse, StatusResponse};
use crate::db::{EventFilter, EventQueryParams};
use crate::ledger::events::EventType;
use crate::{sync, AppState};

/// Maximum number of ledgers to backfill per request.
const BACKFILL_BATCH_SIZE: u32 = 100;

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
    ledger: Option<u32>,
    #[serde(default)]
    tx: Option<String>,
    #[serde(default)]
    filters: Vec<EventFilter>,
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

    let after = multi
        .get("after")
        .and_then(|v| v.first())
        .cloned();

    let ledger = parse_u32_multi(&multi, "ledger")?;

    let tx = multi
        .get("tx")
        .and_then(|v| v.first())
        .cloned();

    // Structured filters (JSON-encoded array)
    let filters: Vec<EventFilter> = match multi.get("filters").and_then(|v| v.first()) {
        Some(json_str) => {
            serde_json::from_str(json_str).map_err(|e| ApiError::BadRequest {
                message: format!("invalid filters JSON: {}", e),
                param: Some("filters".to_string()),
            })?
        }
        None => Vec::new(),
    };

    let req = ListEventsRequest {
        limit,
        after,
        ledger,
        tx,
        filters,
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

/// Fetch and cache historical ledgers on demand, starting at `target_ledger`.
#[tracing::instrument(skip(state))]
async fn backfill_if_needed(state: &AppState, target_ledger: u32) {
    // Cap the range at the latest synced ledger — don't try to fetch future ledgers
    // since the sync task will handle those.
    let latest = state.store.latest_ledger_sequence().ok().flatten().unwrap_or(0);
    if target_ledger > latest {
        return;
    }
    let range = BACKFILL_BATCH_SIZE.min(latest.saturating_sub(target_ledger) + 1);
    let uncached = state.store.find_uncached_ledgers(target_ledger, range).unwrap_or_default();

    if uncached.is_empty() {
        return;
    }

    tracing::debug!(count = uncached.len(), "backfilling uncached ledgers");

    // Fetch all uncached ledgers concurrently
    let futures: Vec<_> = uncached
        .iter()
        .map(|&seq| sync::fetch_and_extract(&state.client, &state.meta_url, &state.config, seq))
        .collect();
    let results = futures::future::join_all(futures).await;

    // Store results directly — no locks needed with the in-memory store.
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
            Err(e) if matches!(e, crate::Error::LedgerNotFound(_)) => {
                // Ledger doesn't exist in S3 — skip
            }
            Err(e) => {
                tracing::warn!(ledger = seq, error = %e, "backfill: failed to fetch ledger");
            }
        }
    }
}

#[tracing::instrument(skip_all, fields(limit = req.limit, ledger = req.ledger, filters = req.filters.len()))]
async fn list_events(
    state: Arc<AppState>,
    req: ListEventsRequest,
) -> Result<Json<ListResponse<Event>>, ApiError> {
    let start = std::time::Instant::now();
    let limit = req.limit.unwrap_or(10);

    if limit == 0 || limit > 100 {
        return Err(ApiError::BadRequest {
            message: "limit must be between 1 and 100".to_string(),
            param: Some("limit".to_string()),
        });
    }

    // Validate and convert cursor from external to internal format if provided.
    let after = if let Some(ref cursor) = req.after {
        // Try decoding as an external (opaque) ID first, fall back to internal format.
        if let Some(internal) = crate::ledger::events::to_internal_id(cursor) {
            Some(internal)
        } else if crate::ledger::events::parse_event_id(cursor).is_some() {
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

    // tx requires a ledger to scope the search
    if req.tx.is_some() && req.ledger.is_none() {
        return Err(ApiError::BadRequest {
            message: "ledger is required when tx is provided".to_string(),
            param: Some("ledger".to_string()),
        });
    }

    // Validate event types within filters
    for filter in &req.filters {
        if let Some(ref t) = filter.event_type {
            t.parse::<EventType>().map_err(|e| ApiError::BadRequest {
                message: e,
                param: Some("filters[].type".to_string()),
            })?;
        }
    }

    // Determine target ledger for on-demand backfill
    let target_ledger = if req.ledger.is_some() {
        req.ledger
    } else if let Some(ref cursor) = after {
        crate::ledger::events::parse_event_id(cursor).map(|(seq, _, _, _, _)| seq)
    } else {
        None
    };
    if let Some(target) = target_ledger {
        backfill_if_needed(&state, target).await;
    }

    let params = EventQueryParams {
        limit,
        after,
        ledger: req.ledger,
        tx: req.tx,
        filters: req.filters,
    };

    let result = state.store.query_events(&params).map_err(|e| ApiError::Internal {
        message: format!("database error: {}", e),
    })?;

    tracing::debug!(events = result.data.len(), has_more = result.has_more, "query complete");

    let events: Vec<Event> = result.data.into_iter().map(Event::from).collect();

    metrics::counter!("api_requests_total", "endpoint" => "events").increment(1);
    metrics::histogram!("api_request_duration_seconds", "endpoint" => "events")
        .record(start.elapsed().as_secs_f64());
    metrics::histogram!("api_events_returned").record(events.len() as f64);

    let response = ListResponse {
        object: "list",
        url: "/events".to_string(),
        has_more: result.has_more,
        next: result.next,
        data: events,
    };

    Ok(Json(response))
}

/// GET /health
#[tracing::instrument(skip_all)]
pub async fn health(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, ApiError> {
    let earliest = state.store.earliest_ledger_sequence().map_err(|e| ApiError::Internal {
        message: format!("database error: {}", e),
    })?;
    let latest = state.store.latest_ledger_sequence().map_err(|e| ApiError::Internal {
        message: format!("database error: {}", e),
    })?;

    let response = StatusResponse {
        status: "ok".to_string(),
        earliest_ledger: earliest,
        latest_ledger: latest,
        network_passphrase: state.config.network_passphrase.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };

    Ok(Json(response))
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
        crate::ledger::events::decode_event_id(&id).ok_or_else(|| ApiError::NotFound {
            message: format!("event not found: {}", id),
        })?;

    // Reconstruct EventPhase; return 404 for invalid (phase, sub) combinations.
    let event_phase = match (phase, sub) {
        (0, 0) => crate::ledger::events::EventPhase::BeforeAllTxs,
        (1, 0) => crate::ledger::events::EventPhase::Operation,
        (1, 1) => crate::ledger::events::EventPhase::AfterTx,
        (2, 0) => crate::ledger::events::EventPhase::AfterAllTxs,
        _ => {
            return Err(ApiError::NotFound {
                message: format!("event not found: {}", id),
            })
        }
    };

    // Reconstruct internal ID for store lookup.
    let internal_id =
        crate::ledger::events::event_id(ledger_seq, event_phase, tx_index, event_index);

    // Backfill the ledger on demand.
    backfill_if_needed(&state, ledger_seq).await;

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

    Ok(Json(event))
}

fn parse_u32_multi(
    params: &HashMap<String, Vec<String>>,
    key: &str,
) -> Result<Option<u32>, ApiError> {
    match params.get(key).and_then(|v| v.first()) {
        Some(v) => v
            .parse::<u32>()
            .map(Some)
            .map_err(|_| ApiError::BadRequest {
                message: format!("{} must be a positive integer", key),
                param: Some(key.to_string()),
            }),
        None => Ok(None),
    }
}
