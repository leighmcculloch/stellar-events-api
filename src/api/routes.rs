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

/// TTL for on-demand backfilled ledgers: 2 hours.
const BACKFILL_TTL_SECONDS: i64 = 2 * 60 * 60;
/// Maximum number of ledgers to backfill per request.
const BACKFILL_BATCH_SIZE: u32 = 100;

/// GET /
pub async fn home() -> axum::response::Html<&'static str> {
    axum::response::Html(HOME_HTML)
}

const HOME_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Stellar Events API</title>
<style>
*,*::before,*::after{box-sizing:border-box}
body{margin:0;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;background:#0f1117;color:#e1e4e8;line-height:1.6}
a{color:#58a6ff;text-decoration:none}
a:hover{text-decoration:underline}
.container{max-width:960px;margin:0 auto;padding:24px 20px}
header{border-bottom:1px solid #30363d;padding-bottom:20px;margin-bottom:32px}
header h1{margin:0 0 6px;font-size:28px;color:#f0f6fc}
header p{margin:0;color:#8b949e;font-size:15px}
h2{font-size:20px;color:#f0f6fc;margin:32px 0 12px;border-bottom:1px solid #21262d;padding-bottom:8px}
h3{font-size:16px;color:#c9d1d9;margin:20px 0 8px}
code{background:#161b22;border:1px solid #30363d;border-radius:4px;padding:2px 6px;font-size:13px;font-family:"SF Mono",Consolas,"Liberation Mono",Menlo,monospace;color:#79c0ff}
pre{background:#161b22;border:1px solid #30363d;border-radius:6px;padding:14px;overflow-x:auto;font-size:13px;font-family:"SF Mono",Consolas,"Liberation Mono",Menlo,monospace;color:#c9d1d9;line-height:1.5}
table{width:100%;border-collapse:collapse;margin:8px 0 16px;font-size:14px}
th,td{text-align:left;padding:8px 12px;border:1px solid #21262d}
th{background:#161b22;color:#8b949e;font-weight:600;font-size:12px;text-transform:uppercase;letter-spacing:.5px}
td code{border:none;padding:0;background:none;color:#79c0ff}
.method{display:inline-block;font-size:12px;font-weight:700;padding:2px 8px;border-radius:4px;margin-right:8px;font-family:"SF Mono",Consolas,monospace}
.method-get{background:#1f6feb22;color:#58a6ff;border:1px solid #1f6feb44}
.method-post{background:#23863622;color:#3fb950;border:1px solid #23863644}
.endpoint-heading{display:flex;align-items:center;margin:20px 0 8px}
.endpoint-heading code{font-size:15px;border:none;background:none;padding:0}
.tabs{display:flex;gap:0;border-bottom:2px solid #21262d;margin-bottom:0}
.tab{padding:8px 16px;cursor:pointer;color:#8b949e;font-size:14px;border:none;background:none;border-bottom:2px solid transparent;margin-bottom:-2px;transition:color .15s,border-color .15s}
.tab:hover{color:#c9d1d9}
.tab.active{color:#f0f6fc;border-bottom-color:#58a6ff}
.tab-panel{display:none;padding:16px 0}
.tab-panel.active{display:block}
.toggle-row{display:flex;align-items:center;gap:12px;margin-bottom:12px}
.toggle-btn{padding:4px 12px;font-size:13px;font-weight:600;border:1px solid #30363d;background:#21262d;color:#8b949e;cursor:pointer;border-radius:4px;font-family:"SF Mono",Consolas,monospace;transition:all .15s}
.toggle-btn.active{background:#1f6feb;border-color:#1f6feb;color:#fff}
.request-area{width:100%;min-height:60px;background:#0d1117;border:1px solid #30363d;border-radius:6px;color:#c9d1d9;font-family:"SF Mono",Consolas,"Liberation Mono",Menlo,monospace;font-size:13px;padding:12px;resize:vertical;line-height:1.5}
.submit-btn{margin-top:10px;padding:6px 20px;font-size:14px;font-weight:600;background:#238636;color:#fff;border:1px solid #2ea04366;border-radius:6px;cursor:pointer;transition:background .15s}
.submit-btn:hover{background:#2ea043}
.submit-btn:disabled{opacity:.5;cursor:not-allowed}
.response-area{margin-top:12px}
.response-area pre{max-height:400px;overflow-y:auto;margin:0}
.response-label{font-size:12px;color:#8b949e;text-transform:uppercase;letter-spacing:.5px;margin-bottom:4px}
.spinner{display:none;width:16px;height:16px;border:2px solid #30363d;border-top-color:#58a6ff;border-radius:50%;animation:spin .6s linear infinite;margin-left:8px;vertical-align:middle}
@keyframes spin{to{transform:rotate(360deg)}}
.filter-table td{vertical-align:top}
section{margin-bottom:40px}
</style>
</head>
<body>
<div class="container">

<header>
  <h1>Stellar Events API</h1>
  <p>Query Stellar contract events via a paginated REST API. Filter by event type, contract ID, transaction hash, and topic patterns.</p>
</header>

<!-- Endpoint Reference -->
<section>
<h2>Endpoint Reference</h2>

<div class="endpoint-heading">
  <span class="method method-get">GET</span>
  <span class="method method-post">POST</span>
  <code>/v1/events</code>
</div>
<p>Retrieve contract events. Both methods accept the same fields — GET uses query parameters, POST uses a JSON body.</p>
<table>
  <tr><th>Parameter</th><th>Type (GET / POST)</th><th>Description</th></tr>
  <tr><td><code>limit</code></td><td>integer</td><td>Number of events to return (1–100, default 10)</td></tr>
  <tr><td><code>after</code></td><td>string</td><td>Cursor for pagination — the event <code>id</code> to start after</td></tr>
  <tr><td><code>ledger</code></td><td>integer</td><td>Only return events from this ledger sequence. Historical ledgers are fetched on demand.</td></tr>
  <tr><td><code>tx</code></td><td>string</td><td>Limit results to events from this transaction hash</td></tr>
  <tr><td><code>filters</code></td><td>JSON string / array</td><td>Filter objects (see below). GET accepts a JSON-encoded string; POST accepts an array.</td></tr>
</table>

<h3>Filter Object</h3>
<table>
  <tr><th>Field</th><th>Type</th><th>Description</th></tr>
  <tr><td><code>type</code></td><td>string</td><td><code>"contract"</code> or <code>"system"</code></td></tr>
  <tr><td><code>contract_id</code></td><td>string</td><td>Stellar contract strkey (C…)</td></tr>
  <tr><td><code>topics</code></td><td>array</td><td>Positional topic matching. Each element is an XDR-JSON ScVal or <code>"*"</code> (wildcard)</td></tr>
</table>

<h3>Response Shape</h3>
<pre>{
  "object": "list",
  "url": "/v1/events",
  "has_more": false,
  "data": [
    {
      "object": "event",
      "id": "0000000000-0000000001",
      "ledger_sequence": 100,
      "ledger_closed_at": "2024-01-01T00:00:00+00:00",
      "tx_hash": "abc123...",
      "type": "contract",
      "contract_id": "CABC...",
      "topics": [...],
      "data": {...}
    }
  ]
}</pre>

<div class="endpoint-heading">
  <span class="method method-get">GET</span>
  <code>/v1/health</code>
</div>
<p>Health check and server status.</p>
<h3>Response Shape</h3>
<pre>{
  "status": "ok",
  "earliest_ledger": 12000,
  "latest_ledger": 12345,
  "network_passphrase": "...",
  "version": "0.1.0"
}</pre>
</section>

<!-- Interactive Explorer -->
<section>
<h2>Interactive Explorer</h2>
<p>Edit the requests below and submit them against this server.</p>

<div class="tabs" id="tabs">
  <button class="tab active" data-tab="latest">Latest events</button>
  <button class="tab" data-tab="bytype">Filter by type</button>
  <button class="tab" data-tab="bytopics">Filter by topics</button>
  <button class="tab" data-tab="bycontract">Filter by contract</button>
  <button class="tab" data-tab="byledger">With ledger</button>
  <button class="tab" data-tab="pagination">With pagination</button>
</div>

<!-- Latest events -->
<div class="tab-panel active" id="panel-latest">
  <div class="toggle-row">
    <button class="toggle-btn active" data-method="GET" data-panel="latest">GET</button>
    <button class="toggle-btn" data-method="POST" data-panel="latest">POST</button>
  </div>
  <div class="request-get" id="req-get-latest">
    <textarea class="request-area" rows="1">/v1/events?limit=5</textarea>
  </div>
  <div class="request-post" id="req-post-latest" style="display:none">
    <textarea class="request-area" rows="3">{
  "limit": 5
}</textarea>
  </div>
  <button class="submit-btn" data-panel="latest">Submit<span class="spinner"></span></button>
  <div class="response-area" id="resp-latest">
    <div class="response-label">Response</div>
    <pre>Click Submit to send a request.</pre>
  </div>
</div>

<!-- Filter by type -->
<div class="tab-panel" id="panel-bytype">
  <div class="toggle-row">
    <button class="toggle-btn active" data-method="GET" data-panel="bytype">GET</button>
    <button class="toggle-btn" data-method="POST" data-panel="bytype">POST</button>
  </div>
  <div class="request-get" id="req-get-bytype">
    <textarea class="request-area" rows="1">/v1/events?limit=5&filters=[{"type":"contract"}]</textarea>
  </div>
  <div class="request-post" id="req-post-bytype" style="display:none">
    <textarea class="request-area" rows="6">{
  "limit": 5,
  "filters": [
    { "type": "contract" }
  ]
}</textarea>
  </div>
  <button class="submit-btn" data-panel="bytype">Submit<span class="spinner"></span></button>
  <div class="response-area" id="resp-bytype">
    <div class="response-label">Response</div>
    <pre>Click Submit to send a request.</pre>
  </div>
</div>

<!-- Filter by topics -->
<div class="tab-panel" id="panel-bytopics">
  <div class="toggle-row">
    <button class="toggle-btn active" data-method="GET" data-panel="bytopics">GET</button>
    <button class="toggle-btn" data-method="POST" data-panel="bytopics">POST</button>
  </div>
  <div class="request-get" id="req-get-bytopics">
    <textarea class="request-area" rows="1">/v1/events?limit=5&filters=[{"type":"contract","topics":[{"symbol":"transfer"}]}]</textarea>
  </div>
  <div class="request-post" id="req-post-bytopics" style="display:none">
    <textarea class="request-area" rows="8">{
  "limit": 5,
  "filters": [
    {
      "type": "contract",
      "topics": [{"symbol": "transfer"}]
    }
  ]
}</textarea>
  </div>
  <button class="submit-btn" data-panel="bytopics">Submit<span class="spinner"></span></button>
  <div class="response-area" id="resp-bytopics">
    <div class="response-label">Response</div>
    <pre>Click Submit to send a request.</pre>
  </div>
</div>

<!-- Filter by contract -->
<div class="tab-panel" id="panel-bycontract">
  <div class="toggle-row">
    <button class="toggle-btn active" data-method="GET" data-panel="bycontract">GET</button>
    <button class="toggle-btn" data-method="POST" data-panel="bycontract">POST</button>
  </div>
  <div class="request-get" id="req-get-bycontract">
    <textarea class="request-area" rows="1">/v1/events?limit=5&filters=[{"contract_id":"CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI"}]</textarea>
  </div>
  <div class="request-post" id="req-post-bycontract" style="display:none">
    <textarea class="request-area" rows="6">{
  "limit": 5,
  "filters": [
    { "contract_id": "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI" }
  ]
}</textarea>
  </div>
  <button class="submit-btn" data-panel="bycontract">Submit<span class="spinner"></span></button>
  <div class="response-area" id="resp-bycontract">
    <div class="response-label">Response</div>
    <pre>Click Submit to send a request.</pre>
  </div>
</div>

<!-- With ledger -->
<div class="tab-panel" id="panel-byledger">
  <div class="toggle-row">
    <button class="toggle-btn active" data-method="GET" data-panel="byledger">GET</button>
    <button class="toggle-btn" data-method="POST" data-panel="byledger">POST</button>
  </div>
  <div class="request-get" id="req-get-byledger">
    <textarea class="request-area" rows="1">/v1/events?limit=5&ledger=58000000</textarea>
  </div>
  <div class="request-post" id="req-post-byledger" style="display:none">
    <textarea class="request-area" rows="4">{
  "limit": 5,
  "ledger": 58000000
}</textarea>
  </div>
  <button class="submit-btn" data-panel="byledger">Submit<span class="spinner"></span></button>
  <div class="response-area" id="resp-byledger">
    <div class="response-label">Response</div>
    <pre>Click Submit to send a request.</pre>
  </div>
</div>

<!-- With pagination -->
<div class="tab-panel" id="panel-pagination">
  <div class="toggle-row">
    <button class="toggle-btn active" data-method="GET" data-panel="pagination">GET</button>
    <button class="toggle-btn" data-method="POST" data-panel="pagination">POST</button>
  </div>
  <div class="request-get" id="req-get-pagination">
    <textarea class="request-area" rows="1">/v1/events?limit=5&after=evt_0058000000_1_0080_0_0000</textarea>
  </div>
  <div class="request-post" id="req-post-pagination" style="display:none">
    <textarea class="request-area" rows="4">{
  "limit": 5,
  "after": "evt_0058000000_1_0080_0_0000"
}</textarea>
  </div>
  <button class="submit-btn" data-panel="pagination">Submit<span class="spinner"></span></button>
  <div class="response-area" id="resp-pagination">
    <div class="response-label">Response</div>
    <pre>Click Submit to send a request.</pre>
  </div>
</div>

</section>
</div>

<script>
(function() {
  // Tabs
  document.getElementById('tabs').addEventListener('click', function(e) {
    var btn = e.target.closest('.tab');
    if (!btn) return;
    document.querySelectorAll('.tab').forEach(function(t) { t.classList.remove('active'); });
    document.querySelectorAll('.tab-panel').forEach(function(p) { p.classList.remove('active'); });
    btn.classList.add('active');
    document.getElementById('panel-' + btn.dataset.tab).classList.add('active');
  });

  // GET/POST toggle
  document.querySelectorAll('.toggle-btn').forEach(function(btn) {
    btn.addEventListener('click', function() {
      var panel = btn.dataset.panel;
      var method = btn.dataset.method;
      document.querySelectorAll('.toggle-btn[data-panel="' + panel + '"]').forEach(function(b) {
        b.classList.remove('active');
      });
      btn.classList.add('active');
      var getEl = document.getElementById('req-get-' + panel);
      var postEl = document.getElementById('req-post-' + panel);
      if (method === 'GET') {
        getEl.style.display = '';
        postEl.style.display = 'none';
      } else {
        getEl.style.display = 'none';
        postEl.style.display = '';
      }
    });
  });

  // Submit
  document.querySelectorAll('.submit-btn').forEach(function(btn) {
    btn.addEventListener('click', function() {
      var panel = btn.dataset.panel;
      var activeToggle = document.querySelector('.toggle-btn.active[data-panel="' + panel + '"]');
      var method = activeToggle ? activeToggle.dataset.method : 'GET';
      var respPre = document.querySelector('#resp-' + panel + ' pre');
      var spinner = btn.querySelector('.spinner');

      btn.disabled = true;
      spinner.style.display = 'inline-block';
      respPre.textContent = 'Loading...';

      var opts = {};
      var url;

      if (method === 'GET') {
        var textarea = document.querySelector('#req-get-' + panel + ' .request-area');
        url = textarea.value.trim();
        opts.method = 'GET';
      } else {
        url = '/v1/events';
        var textarea = document.querySelector('#req-post-' + panel + ' .request-area');
        var body = textarea.value.trim();
        opts.method = 'POST';
        opts.headers = { 'Content-Type': 'application/json' };
        opts.body = body;
      }

      fetch(url, opts)
        .then(function(r) { return r.text(); })
        .then(function(text) {
          try {
            var json = JSON.parse(text);
            respPre.textContent = JSON.stringify(json, null, 2);
          } catch(e) {
            respPre.textContent = text;
          }
        })
        .catch(function(err) {
          respPre.textContent = 'Error: ' + err.message;
        })
        .finally(function() {
          btn.disabled = false;
          spinner.style.display = 'none';
        });
    });
  });
})();
</script>
</body>
</html>"##;


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

/// JSON request body for POST /v1/events.
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

/// GET /v1/events
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

/// POST /v1/events
pub async fn list_events_post(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListEventsRequest>,
) -> Result<impl IntoResponse, ApiError> {
    list_events(state, req).await
}

/// Fetch and cache historical ledgers on demand, starting at `target_ledger`.
async fn backfill_if_needed(state: &AppState, target_ledger: u32) {
    // Use the reader to check cache (avoids contention with sync writer).
    // Cap the range at the latest synced ledger — don't try to fetch future ledgers
    // since the sync task will handle those.
    let uncached: Vec<u32> = {
        let db = match state.reader.lock() {
            Ok(db) => db,
            Err(_) => return,
        };
        let latest = db.latest_ledger_sequence().ok().flatten().unwrap_or(0);
        if target_ledger > latest {
            return;
        }
        let range = BACKFILL_BATCH_SIZE.min(latest.saturating_sub(target_ledger) + 1);
        db.find_uncached_ledgers(target_ledger, range)
            .unwrap_or_default()
    };

    if uncached.is_empty() {
        return;
    }

    // Fetch all uncached ledgers concurrently
    let futures: Vec<_> = uncached
        .iter()
        .map(|&seq| sync::fetch_and_extract(&state.client, &state.meta_url, &state.config, seq))
        .collect();
    let results = futures::future::join_all(futures).await;

    // Store results
    let db = match state.writer.lock() {
        Ok(db) => db,
        Err(_) => return,
    };
    for (i, result) in results.into_iter().enumerate() {
        let seq = uncached[i];
        match result {
            Ok(events) => {
                if let Err(e) = db.insert_events(&events) {
                    tracing::warn!(ledger = seq, error = %e, "backfill: failed to insert events");
                    continue;
                }
                if let Err(e) = db.record_ledger_cached(seq, BACKFILL_TTL_SECONDS) {
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

async fn list_events(
    state: Arc<AppState>,
    req: ListEventsRequest,
) -> Result<Json<ListResponse<Event>>, ApiError> {
    let limit = req.limit.unwrap_or(10);

    if limit == 0 || limit > 100 {
        return Err(ApiError::BadRequest {
            message: "limit must be between 1 and 100".to_string(),
            param: Some("limit".to_string()),
        });
    }

    // Validate cursor if provided
    if let Some(ref cursor) = req.after {
        if crate::ledger::events::parse_event_id(cursor).is_none() {
            return Err(ApiError::BadRequest {
                message: format!("invalid cursor: {}", cursor),
                param: Some("after".to_string()),
            });
        }
    }

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
    } else if let Some(ref cursor) = req.after {
        crate::ledger::events::parse_event_id(cursor).map(|(seq, _, _)| seq)
    } else {
        None
    };
    if let Some(target) = target_ledger {
        backfill_if_needed(&state, target).await;
    }

    let params = EventQueryParams {
        limit,
        after: req.after,
        ledger: req.ledger,
        tx: req.tx,
        filters: req.filters,
    };

    let result = {
        let db = state.reader.lock().map_err(|_| ApiError::Internal {
            message: "database lock poisoned".to_string(),
        })?;
        db.query_events(&params).map_err(|e| ApiError::Internal {
            message: format!("database error: {}", e),
        })?
    };

    let events: Vec<Event> = result.data.into_iter().map(Event::from).collect();

    let response = ListResponse {
        object: "list",
        url: "/v1/events".to_string(),
        has_more: result.has_more,
        data: events,
    };

    Ok(Json(response))
}

/// GET /v1/health
pub async fn health(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, ApiError> {
    let (earliest, latest) = {
        let db = state.reader.lock().map_err(|_| ApiError::Internal {
            message: "database lock poisoned".to_string(),
        })?;
        let earliest = db
            .earliest_ledger_sequence()
            .map_err(|e| ApiError::Internal {
                message: format!("database error: {}", e),
            })?;
        let latest = db
            .latest_ledger_sequence()
            .map_err(|e| ApiError::Internal {
                message: format!("database error: {}", e),
            })?;
        (earliest, latest)
    };

    let response = StatusResponse {
        status: "ok".to_string(),
        earliest_ledger: earliest,
        latest_ledger: latest,
        network_passphrase: state.config.network_passphrase.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };

    Ok(Json(response))
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
