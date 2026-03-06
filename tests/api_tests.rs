use std::sync::Arc;
use std::time::Duration;

use stellar_events_api::api;
use stellar_events_api::db::EventStore;
use stellar_events_api::ledger::event_id::EventPhase;
use stellar_events_api::ledger::events::{EventType, ExtractedEvent};
use stellar_events_api::ledger::path::StoreConfig;
use stellar_events_api::AppState;

/// Helper: start a test server and return its base URL.
async fn start_test_server(events: Vec<ExtractedEvent>) -> String {
    let store = EventStore::new(24 * 60 * 60);
    if !events.is_empty() {
        store
            .insert_events(events)
            .expect("failed to insert events");
    }

    let state = Arc::new(AppState {
        store,
        config: StoreConfig::default(),
        meta_url: String::new(),
        client: reqwest::Client::new(),
    });

    let app = api::router(state, None);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind");
    let addr = listener.local_addr().expect("failed to get addr");
    let base_url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Give the server a moment to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    base_url
}

/// Helper: create test events with simple topics, all on the same ledger.
fn make_test_events(count: usize, ledger: u32) -> Vec<ExtractedEvent> {
    (0..count)
        .map(|i| ExtractedEvent {
            ledger_sequence: ledger,
            ledger_closed_at: 1700000000,
            phase: EventPhase::Operation,
            tx_index: i as u32,
            event_index: 0,
            tx_hash: format!("{:064x}", i),
            contract_id: Some(format!(
                "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA{}",
                i
            )),
            event_type: EventType::Contract,
            topics_xdr_json: vec![serde_json::json!({"symbol": "transfer"})],
            data_xdr_json: serde_json::json!({"amount": i * 100}),
        })
        .collect()
}

/// Create events with varied types, contracts, and XDR-JSON ScVal topics.
/// All events are on ledger 100 so that `q=ledger:100` returns the full set.
fn make_multi_type_events() -> Vec<ExtractedEvent> {
    vec![
        // Event 0: transfer from addr_A to addr_B on contract CA
        ExtractedEvent {
            ledger_sequence: 100,
            ledger_closed_at: 1700000000,
            phase: EventPhase::Operation,
            tx_index: 0,
            event_index: 0,
            tx_hash: "a".repeat(64),
            contract_id: Some(
                "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
            ),
            event_type: EventType::Contract,
            topics_xdr_json: vec![
                serde_json::json!({"symbol": "transfer"}),
                serde_json::json!({"address": "GABC"}),
                serde_json::json!({"address": "GDEF"}),
            ],
            data_xdr_json: serde_json::json!({"i128": {"hi": 0, "lo": 100}}),
        },
        // Event 1: system event (no contract)
        ExtractedEvent {
            ledger_sequence: 100,
            ledger_closed_at: 1700000000,
            phase: EventPhase::Operation,
            tx_index: 0,
            event_index: 1,
            tx_hash: "a".repeat(64),
            contract_id: None,
            event_type: EventType::System,
            topics_xdr_json: vec![serde_json::json!({"symbol": "core_metrics"})],
            data_xdr_json: serde_json::json!({}),
        },
        // Event 2: transfer on contract CB
        ExtractedEvent {
            ledger_sequence: 100,
            ledger_closed_at: 1700000000,
            phase: EventPhase::Operation,
            tx_index: 1,
            event_index: 0,
            tx_hash: "b".repeat(64),
            contract_id: Some(
                "CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".to_string(),
            ),
            event_type: EventType::Contract,
            topics_xdr_json: vec![
                serde_json::json!({"symbol": "transfer"}),
                serde_json::json!({"address": "GCCC"}),
                serde_json::json!({"address": "GDDD"}),
            ],
            data_xdr_json: serde_json::json!({"i128": {"hi": 0, "lo": 200}}),
        },
        // Event 3: mint on contract CA
        ExtractedEvent {
            ledger_sequence: 100,
            ledger_closed_at: 1700000000,
            phase: EventPhase::Operation,
            tx_index: 2,
            event_index: 0,
            tx_hash: "c".repeat(64),
            contract_id: Some(
                "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
            ),
            event_type: EventType::Contract,
            topics_xdr_json: vec![
                serde_json::json!({"symbol": "mint"}),
                serde_json::json!({"address": "GABC"}),
            ],
            data_xdr_json: serde_json::json!({"i128": {"hi": 0, "lo": 500}}),
        },
        // Event 4: diagnostic on contract CA
        ExtractedEvent {
            ledger_sequence: 100,
            ledger_closed_at: 1700000000,
            phase: EventPhase::Operation,
            tx_index: 2,
            event_index: 1,
            tx_hash: "c".repeat(64),
            contract_id: Some(
                "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
            ),
            event_type: EventType::Diagnostic,
            topics_xdr_json: vec![serde_json::json!({"symbol": "diag"})],
            data_xdr_json: serde_json::json!({}),
        },
    ]
}

fn q_param(q: &str) -> String {
    urlencoding::encode(q).to_string()
}

// --- Basic list and pagination ---

#[tokio::test]
async fn test_list_events_empty() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/events", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "list");
    assert_eq!(body["url"], "/events");
    assert!(body["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_list_events_with_data() {
    let events = make_test_events(5, 1000);
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "{}/events?limit=3&q={}",
            base_url,
            q_param("ledger:1000")
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "list");
    assert_eq!(body["data"].as_array().unwrap().len(), 3);

    let first = &body["data"][0];
    assert_eq!(first["object"], "event");
    assert!(first["id"].as_str().unwrap().starts_with("evt_"));
    assert!(first["ledger"].is_number());
    assert!(first["at"].is_string());
    assert!(first["tx"].is_string());
    assert!(first["type"].is_string());
}

#[tokio::test]
async fn test_pagination_with_before() {
    let events = make_test_events(5, 1000);
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // First page (newest first, no cursor)
    let resp = client
        .get(format!(
            "{}/events?limit=2&q={}",
            base_url,
            q_param("ledger:1000")
        ))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    let next_cursor = body["next"].as_str().unwrap().to_string();

    // Second page using `before` with the next cursor
    let resp = client
        .get(format!(
            "{}/events?limit=2&before={}",
            base_url, next_cursor
        ))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    let next_cursor = body["next"].as_str().unwrap().to_string();

    // Third page (last)
    let resp = client
        .get(format!(
            "{}/events?limit=2&before={}",
            base_url, next_cursor
        ))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_default_limit() {
    let events = make_test_events(15, 1000);
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/events?q={}", base_url, q_param("ledger:1000")))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 10);
}

#[tokio::test]
async fn test_default_starts_at_latest_ledger() {
    let events = make_test_events(5, 1000);
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // No ledger or after — should default to latest ledger (1000, since all events are on it)
    let resp = client
        .get(format!("{}/events", base_url))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 5);
    assert_eq!(data[0]["ledger"], 1000);
}

// --- Ledger sequence filter ---

#[tokio::test]
async fn test_ledger_filter() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // ledger:100 returns all 5 events (all on ledger 100)
    let resp = client
        .get(format!("{}/events?q={}", base_url, q_param("ledger:100")))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 5);
    for evt in data {
        assert_eq!(evt["ledger"].as_u64().unwrap(), 100);
    }
}

#[tokio::test]
async fn test_ledger_filter_no_match() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // ledger:999 returns no events (no data on that ledger)
    let resp = client
        .get(format!("{}/events?q={}", base_url, q_param("ledger:999")))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 0);
}

// --- Tx hash filter ---

#[tokio::test]
async fn test_filter_by_tx_hash() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let tx_hash = "b".repeat(64);
    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param(&format!("ledger:100 tx:{}", tx_hash))
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["tx"], tx_hash);
}

#[tokio::test]
async fn test_tx_without_ledger_returns_error() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param(&format!("tx:{}", "a".repeat(64)))
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("ledger is required when tx is provided"));
}

// --- Validation and errors ---

#[tokio::test]
async fn test_invalid_limit() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/events?limit=0", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let resp = client
        .get(format!("{}/events?limit=101", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_invalid_cursor() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/events?after=invalid_cursor", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["type"], "invalid_request_error");
}

#[tokio::test]
async fn test_error_response_format() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/events?limit=abc", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("error").is_some());
    let error = &body["error"];
    assert!(error.get("type").is_some());
    assert!(error.get("message").is_some());
}

// --- POST ---

#[tokio::test]
async fn test_post_list_events() {
    let events = make_test_events(3, 1000);
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({
            "limit": 2,
            "q": "ledger:1000"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "list");
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_post_invalid_limit() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({"limit": 0}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_post_empty_body() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "list");
}

// --- Descending order and before cursor ---

#[tokio::test]
async fn test_default_returns_descending_order() {
    let events = make_test_events(5, 1000);
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/events?q={}", base_url, q_param("ledger:1000")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 5);

    // Each event has a unique tx_index (0..4). In descending order,
    // tx hashes should go from tx_index=4 to tx_index=0.
    // make_test_events creates tx_hash as format!("{:064x}", i) where i is the index.
    let tx_hashes: Vec<&str> = data.iter().map(|e| e["tx"].as_str().unwrap()).collect();
    assert_eq!(tx_hashes[0], format!("{:064x}", 4));
    assert_eq!(tx_hashes[1], format!("{:064x}", 3));
    assert_eq!(tx_hashes[2], format!("{:064x}", 2));
    assert_eq!(tx_hashes[3], format!("{:064x}", 1));
    assert_eq!(tx_hashes[4], format!("{:064x}", 0));
}

#[tokio::test]
async fn test_before_cursor_paginates_backward() {
    let events = make_test_events(5, 1000);
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // Get all events to know their IDs (returned desc).
    let resp = client
        .get(format!("{}/events?q={}", base_url, q_param("ledger:1000")))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let all_ids: Vec<String> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(all_ids.len(), 5);
    // all_ids[0] is newest (tx4), all_ids[4] is oldest (tx0)

    // Use the 3rd event (middle) as `before` cursor with limit=2.
    // Should return events older than all_ids[2], in desc order.
    let resp = client
        .get(format!("{}/events?before={}&limit=2", base_url, all_ids[2]))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(data[0]["id"].as_str().unwrap(), all_ids[3]);
    assert_eq!(data[1]["id"].as_str().unwrap(), all_ids[4]);
}

#[tokio::test]
async fn test_before_cursor_pagination_continuation() {
    let events = make_test_events(5, 1000);
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // First page: no cursor, limit=2 → newest 2 events.
    let resp = client
        .get(format!(
            "{}/events?limit=2&q={}",
            base_url,
            q_param("ledger:1000")
        ))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    let next_cursor = body["next"].as_str().unwrap().to_string();

    // Second page: use next as `before`.
    let resp = client
        .get(format!(
            "{}/events?before={}&limit=2",
            base_url, next_cursor
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);

    // Third page
    let next_cursor = body["next"].as_str().unwrap().to_string();
    let resp = client
        .get(format!(
            "{}/events?before={}&limit=2",
            base_url, next_cursor
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
}

#[tokio::test]
async fn test_after_cursor_returns_newer_events_in_desc() {
    let events = make_test_events(5, 1000);
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // Get all events to know their IDs (returned desc).
    let resp = client
        .get(format!("{}/events?q={}", base_url, q_param("ledger:1000")))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let all_ids: Vec<String> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(all_ids.len(), 5);
    // all_ids[0] is newest, all_ids[4] is oldest

    // Use the oldest event as `after` cursor with limit=3.
    // Should return events newer than all_ids[4], in desc order.
    let resp = client
        .get(format!("{}/events?after={}&limit=3", base_url, all_ids[4]))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 3);
    // Should be in desc order: all_ids[1], all_ids[2], all_ids[3]
    // (3 events newer than oldest, returned newest-first)
    assert_eq!(data[0]["id"].as_str().unwrap(), all_ids[1]);
    assert_eq!(data[1]["id"].as_str().unwrap(), all_ids[2]);
    assert_eq!(data[2]["id"].as_str().unwrap(), all_ids[3]);
}

#[tokio::test]
async fn test_after_and_before_together_returns_400() {
    let events = make_test_events(2, 1000);
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "{}/events?limit=2&q={}",
            base_url,
            q_param("ledger:1000")
        ))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    let id0 = data[0]["id"].as_str().unwrap();
    let id1 = data[1]["id"].as_str().unwrap();

    let resp = client
        .get(format!("{}/events?after={}&before={}", base_url, id0, id1))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("after and before cannot both be provided"));
}

// --- Response shape ---

#[tokio::test]
async fn test_list_envelope_consistency() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/events", base_url))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();

    assert!(body.get("object").is_some());
    assert!(body.get("url").is_some());
    assert!(body.get("data").is_some());
    assert_eq!(body.as_object().unwrap().len(), 3);
}

#[tokio::test]
async fn test_event_fields_complete() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/events?limit=1", base_url))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let event = &body["data"][0];

    assert!(event["id"].is_string());
    assert_eq!(event["object"], "event");
    assert!(event["type"].is_string());
    assert!(event["ledger"].is_number());
    assert!(event["at"].is_string());
    assert!(event["tx"].is_string());
    assert!(!event["topics"].is_null());
    assert!(!event["data"].is_null());
}

#[tokio::test]
async fn test_status_endpoint() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/health", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["network_passphrase"].is_string());
    assert!(body["cached_ledgers"].is_number());
}

// --- q= filter tests ---

#[tokio::test]
async fn test_q_filter_by_type() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param("ledger:100 type:system")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["type"], "system");
}

#[tokio::test]
async fn test_q_filter_by_contract() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param(
                "ledger:100 contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
            )
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Events 0, 3, 4 are on contract CA
    assert_eq!(data.len(), 3);
    for evt in data {
        assert_eq!(
            evt["contract"],
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        );
    }
}

#[tokio::test]
async fn test_q_filter_by_topic() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param(r#"ledger:100 topic0:{"symbol":"transfer"}"#)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Events 0 and 2 have {"symbol":"transfer"} at position 0
    assert_eq!(data.len(), 2);
}

#[tokio::test]
async fn test_q_filter_topic_with_wildcard() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // topic0 = transfer, topic1 = wildcard (gap), topic2 = GDEF
    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param(r#"ledger:100 topic0:{"symbol":"transfer"} topic2:{"address":"GDEF"}"#)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Only event 0 matches (transfer, GABC, GDEF)
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["topics"][2]["address"], "GDEF");
}

#[tokio::test]
async fn test_q_filter_or_logic() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // Two OR groups: CA with transfer OR CB with transfer
    let q = r#"(ledger:100 contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA topic0:{"symbol":"transfer"}) OR (ledger:100 contract:CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB topic0:{"symbol":"transfer"})"#;
    let resp = client
        .get(format!("{}/events?q={}", base_url, q_param(q)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Event 0 (CA transfer) and event 2 (CB transfer)
    assert_eq!(data.len(), 2);
}

#[tokio::test]
async fn test_q_filter_and_logic() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // Single AND group: contract CA AND type contract AND topic[0] = mint
    let q = r#"ledger:100 contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA type:contract topic0:{"symbol":"mint"}"#;
    let resp = client
        .get(format!("{}/events?q={}", base_url, q_param(q)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Only event 3 matches
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["ledger"], 100);
}

#[tokio::test]
async fn test_q_filter_combined_with_pagination() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let qp = q_param("ledger:100 type:contract");

    // First page: limit=1
    let resp = client
        .get(format!("{}/events?limit=1&q={}", base_url, qp))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    let first_id = data[0]["id"].as_str().unwrap().to_string();

    // Second page using before cursor
    let next_cursor = body["next"].as_str().unwrap();
    let resp = client
        .get(format!(
            "{}/events?limit=1&before={}&q={}",
            base_url, next_cursor, qp
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    // Must be a different event
    assert_ne!(data[0]["id"].as_str().unwrap(), first_id);
}

#[tokio::test]
async fn test_q_filter_combined_with_ledger() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param(r#"ledger:100 topic0:{"symbol":"transfer"}"#)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Events 0 and 2 have {"symbol":"transfer"} topic
    assert_eq!(data.len(), 2);
}

#[tokio::test]
async fn test_q_filter_paren_group() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let q = r#"ledger:100 (contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA OR contract:CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB) topic0:{"symbol":"transfer"}"#;
    let resp = client
        .get(format!("{}/events?q={}", base_url, q_param(q)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Events 0 and 2 (transfer on CA or CB)
    assert_eq!(data.len(), 2);
}

#[tokio::test]
async fn test_q_filter_invalid_syntax() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/events?q={}", base_url, q_param("foo:bar")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert_eq!(body["error"]["code"], "invalid_parameter");
    assert_eq!(body["error"]["param"], "q");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("unknown key"));
}

#[tokio::test]
async fn test_q_filter_post_json() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({
            "q": "ledger:100 type:system"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["type"], "system");
}

#[tokio::test]
async fn test_no_q_returns_all() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/events?q={}", base_url, q_param("ledger:100")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn test_q_filter_url_encoded() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // Manually URL-encode: ledger:100 type:contract topic0:{"symbol":"transfer"}
    let resp = client
        .get(format!(
            "{}/events?q=ledger%3A100%20type%3Acontract%20topic0%3A%7B%22symbol%22%3A%22transfer%22%7D",
            base_url
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Events 0 and 2 are contract type with transfer topic
    assert_eq!(data.len(), 2);
}

#[tokio::test]
async fn test_q_filter_empty_value() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/events?q=", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["param"], "q");
}

#[tokio::test]
async fn test_q_filter_diagnostic() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param("ledger:100 type:diagnostic")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["type"], "diagnostic");
}

#[tokio::test]
async fn test_q_filter_topic_gap() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // topic0=transfer, topic1=wildcard, topic2=GDDD -> event 2
    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param(r#"ledger:100 topic0:{"symbol":"transfer"} topic2:{"address":"GDDD"}"#)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["topics"][2]["address"], "GDDD");
}

#[tokio::test]
async fn test_q_filter_three_way_or() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param("ledger:100 (type:contract OR type:system OR type:diagnostic)")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // All 5 events match one of the three types
    assert_eq!(data.len(), 5);
}

#[tokio::test]
async fn test_q_filter_parse_error_format() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param("(type:contract")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("error").is_some());
    let error = &body["error"];
    assert_eq!(error["type"], "invalid_request_error");
    assert_eq!(error["param"], "q");
    assert!(error.get("message").is_some());
    assert!(error.get("code").is_some());
}

#[tokio::test]
async fn test_q_filter_no_match() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // CB has no mint event
    let q = r#"ledger:100 contract:CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB topic0:{"symbol":"mint"}"#;
    let resp = client
        .get(format!("{}/events?q={}", base_url, q_param(q)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 0);
}

// --- topic (any position) integration tests ---

#[tokio::test]
async fn test_q_filter_topic_any_position() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // topic:{"address":"GABC"} should match events that have GABC in any topic position.
    // Event 0 has GABC at topic1, Event 3 has GABC at topic1.
    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param(r#"ledger:100 topic:{"address":"GABC"}"#)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
}

#[tokio::test]
async fn test_q_filter_topic_any_with_type() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // topic:{"symbol":"transfer"} matches events 0 and 2 (both have transfer at topic0).
    // Adding type:contract should still match both (both are contract type).
    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param(r#"ledger:100 type:contract topic:{"symbol":"transfer"}"#)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
}

#[tokio::test]
async fn test_q_filter_topic_any_multiple_and() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // topic:{"symbol":"transfer"} AND topic:{"address":"GABC"} — both must be present.
    // Event 0 has [transfer, GABC, GDEF] — matches.
    // Event 2 has [transfer, GCCC, GDDD] — no GABC, doesn't match.
    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param(r#"ledger:100 topic:{"symbol":"transfer"} topic:{"address":"GABC"}"#)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
}

#[tokio::test]
async fn test_q_filter_topic_any_no_match() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param(r#"ledger:100 topic:{"symbol":"nonexistent"}"#)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 0);
}

// --- ledger: and tx: query parser tests ---

#[tokio::test]
async fn test_q_ledger_invalid_value() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/events?q={}", base_url, q_param("ledger:abc")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("invalid value"));
}

#[tokio::test]
async fn test_q_ledger_with_tx() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let tx_hash = "a".repeat(64);
    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param(&format!("ledger:100 tx:{}", tx_hash))
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Events 0 and 1 share tx_hash "aaa..."
    assert_eq!(data.len(), 2);
    for evt in data {
        assert_eq!(evt["tx"], tx_hash);
    }
}

// --- POST JSON query tests ---

#[tokio::test]
async fn test_post_json_query_single() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({
            "q": {"and": [{"ledger": 100}, {"type": "contract"}]}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Events 0, 2, 3 are contract type on ledger 100
    assert_eq!(data.len(), 3);
    for evt in data {
        assert_eq!(evt["type"], "contract");
    }
}

#[tokio::test]
async fn test_post_json_query_and() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({
            "q": {"and": [
                {"ledger": 100},
                {"type": "contract"},
                {"contract": "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}
            ]}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Events 0 and 3 are contract type on contract CA
    assert_eq!(data.len(), 2);
    for evt in data {
        assert_eq!(evt["type"], "contract");
        assert_eq!(
            evt["contract"],
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        );
    }
}

#[tokio::test]
async fn test_post_json_query_or() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({
            "q": {"and": [
                {"ledger": 100},
                {"or": [{"type": "contract"}, {"type": "system"}]}
            ]}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Events 0, 1, 2, 3 are contract or system (event 4 is diagnostic)
    assert_eq!(data.len(), 4);
}

#[tokio::test]
async fn test_post_json_query_nested() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({
            "q": {"and": [
                {"ledger": 100},
                {"or": [{"type": "contract"}, {"type": "system"}]},
                {"contract": "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}
            ]}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Only contract-type events on contract CA: events 0 and 3
    // (system event 1 has no contract so doesn't match contract filter)
    assert_eq!(data.len(), 2);
}

#[tokio::test]
async fn test_post_json_query_invalid_type() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({
            "q": {"type": "bogus"}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_post_json_query_tx_requires_ledger() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({
            "q": {"tx": "abc"}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_post_json_query_unknown_key() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({
            "q": {"foo": "bar"}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_post_json_query_multi_key() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({
            "q": {"type": "contract", "contract": "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_post_json_query_topic() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({
            "q": {"and": [
                {"ledger": 100},
                {"topic0": {"symbol": "transfer"}}
            ]}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Events 0 and 2 have transfer at topic0
    assert_eq!(data.len(), 2);
}

#[tokio::test]
async fn test_post_json_query_string_still_works() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // String q in POST should still work
    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({
            "q": "ledger:100 type:system"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["type"], "system");
}

// --- Schema endpoint ---

#[tokio::test]
async fn test_schema_endpoint() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/schema", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/schema+json"
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("$schema").is_some());
    assert!(body.get("$defs").is_some());
    assert!(body["$defs"].get("QueryExpr").is_some());
}

// --- Cross-ledger progressive search tests ---

/// Create test events spread across three ledgers (100, 101, 102), 2 events each.
fn make_cross_ledger_events() -> Vec<ExtractedEvent> {
    let mut events = Vec::new();
    for ledger in [100u32, 101, 102] {
        for i in 0..2u32 {
            events.push(ExtractedEvent {
                ledger_sequence: ledger,
                ledger_closed_at: 1_700_000_000 + i64::from(ledger) * 100,
                phase: EventPhase::Operation,
                tx_index: i,
                event_index: 0,
                tx_hash: format!("{:03}_{:061x}", ledger, i),
                contract_id: Some(if ledger <= 101 {
                    "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string()
                } else {
                    "CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".to_string()
                }),
                event_type: EventType::Contract,
                topics_xdr_json: vec![serde_json::json!({"symbol": "transfer"})],
                data_xdr_json: serde_json::json!({"amount": ledger * 10 + i}),
            });
        }
    }
    events
}

#[tokio::test]
async fn test_cross_ledger_backward_no_cursor() {
    let events = make_cross_ledger_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // No cursor, limit=3 — should get newest 3 events (desc order).
    let resp = client
        .get(format!("{}/events?limit=3", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 3);
    // Newest first: 2 from ledger 102, then 1 from ledger 101.
    assert_eq!(data[0]["ledger"], 102);
    assert_eq!(data[1]["ledger"], 102);
    assert_eq!(data[2]["ledger"], 101);
}

#[tokio::test]
async fn test_cross_ledger_backward_pagination() {
    let events = make_cross_ledger_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // Page 1: newest 2
    let resp = client
        .get(format!("{}/events?limit=2", base_url))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(data[0]["ledger"], 102);
    assert_eq!(data[1]["ledger"], 102);
    let next = body["next"].as_str().unwrap().to_string();

    // Page 2
    let resp = client
        .get(format!("{}/events?limit=2&before={}", base_url, next))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(data[0]["ledger"], 101);
    assert_eq!(data[1]["ledger"], 101);
    let next = body["next"].as_str().unwrap().to_string();

    // Page 3
    let resp = client
        .get(format!("{}/events?limit=2&before={}", base_url, next))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(data[0]["ledger"], 100);
    assert_eq!(data[1]["ledger"], 100);
}

#[tokio::test]
async fn test_cross_ledger_forward() {
    let events = make_cross_ledger_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // Get all events to find the oldest ID.
    let resp = client
        .get(format!("{}/events?limit=10", base_url))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 6);
    let oldest_id = data.last().unwrap()["id"].as_str().unwrap().to_string();

    // Query with after=oldest, limit=3.
    let resp = client
        .get(format!("{}/events?after={}&limit=3", base_url, oldest_id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 3);
    // Results in descending order.
    let ledgers: Vec<u64> = data.iter().map(|e| e["ledger"].as_u64().unwrap()).collect();
    assert!(ledgers[0] >= ledgers[1]);
    assert!(ledgers[1] >= ledgers[2]);
}

#[tokio::test]
async fn test_cross_ledger_with_filter() {
    let events = make_cross_ledger_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // Filter by contract CA — only ledgers 100 and 101 have contract CA.
    let resp = client
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param("contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 4);
    for evt in data {
        assert_eq!(
            evt["contract"],
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        );
    }
}

#[tokio::test]
async fn test_cross_ledger_all_events() {
    let events = make_cross_ledger_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // Should return all 6 events across 3 ledgers.
    let resp = client
        .get(format!("{}/events?limit=10", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 6);
    // Descending order: ledger 102, 102, 101, 101, 100, 100.
    assert_eq!(data[0]["ledger"], 102);
    assert_eq!(data[1]["ledger"], 102);
    assert_eq!(data[2]["ledger"], 101);
    assert_eq!(data[3]["ledger"], 101);
    assert_eq!(data[4]["ledger"], 100);
    assert_eq!(data[5]["ledger"], 100);
}
