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
/// All events are on ledger 100 so that `ledger=100` returns the full set.
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

fn filters_param(filters: &serde_json::Value) -> String {
    urlencoding::encode(&filters.to_string()).to_string()
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
        .get(format!("{}/events?ledger=1000&limit=3", base_url))
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
        .get(format!("{}/events?ledger=1000&limit=2", base_url))
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
        .get(format!("{}/events?ledger=1000", base_url))
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

    // ledger=100 returns all 5 events (all on ledger 100)
    let resp = client
        .get(format!("{}/events?ledger=100", base_url))
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

    // ledger=999 returns no events (no data on that ledger)
    let resp = client
        .get(format!("{}/events?ledger=999", base_url))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 0);
}

// --- Structured filters ---

#[tokio::test]
async fn test_filter_by_contract_id() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let f = serde_json::json!([{
        "contract": "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
    }]);
    let resp = client
        .get(format!(
            "{}/events?ledger=100&filters={}",
            base_url,
            filters_param(&f)
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
async fn test_filter_by_type() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let f = serde_json::json!([{"type": "system"}]);
    let resp = client
        .get(format!(
            "{}/events?ledger=100&filters={}",
            base_url,
            filters_param(&f)
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
async fn test_filter_by_tx_hash() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let tx_hash = "b".repeat(64);
    let resp = client
        .get(format!("{}/events?ledger=100&tx={}", base_url, tx_hash))
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
        .get(format!("{}/events?tx={}", base_url, "a".repeat(64)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["param"], "ledger");
}

#[tokio::test]
async fn test_filter_by_topic_positional() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // Match events where topic[0] is {"symbol":"transfer"}
    let f = serde_json::json!([{"topics": [{"symbol": "transfer"}]}]);
    let resp = client
        .get(format!(
            "{}/events?ledger=100&filters={}",
            base_url,
            filters_param(&f)
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
async fn test_filter_topic_with_wildcard() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // topic[0] = transfer, topic[1] = anything, topic[2] = GDEF
    let f = serde_json::json!([{"topics": [{"symbol": "transfer"}, null, {"address": "GDEF"}]}]);
    let resp = client
        .get(format!(
            "{}/events?ledger=100&filters={}",
            base_url,
            filters_param(&f)
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
async fn test_filter_topic_too_few_positions() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // No event has 4 topics
    let f =
        serde_json::json!([{"topics": [{"symbol": "transfer"}, null, null, {"symbol": "extra"}]}]);
    let resp = client
        .get(format!(
            "{}/events?ledger=100&filters={}",
            base_url,
            filters_param(&f)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_filters_or_logic() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // Two filters OR'd: CA with transfer OR CB with transfer
    let f = serde_json::json!([
        {
            "contract": "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "topics": [{"symbol": "transfer"}]
        },
        {
            "contract": "CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
            "topics": [{"symbol": "transfer"}]
        }
    ]);
    let resp = client
        .get(format!(
            "{}/events?ledger=100&filters={}",
            base_url,
            filters_param(&f)
        ))
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
async fn test_filters_and_logic_within() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // Single filter: contract CA AND type contract AND topic[0] = mint
    let f = serde_json::json!([{
        "contract": "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "type": "contract",
        "topics": [{"symbol": "mint"}]
    }]);
    let resp = client
        .get(format!(
            "{}/events?ledger=100&filters={}",
            base_url,
            filters_param(&f)
        ))
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
async fn test_filters_combined_with_pagination() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // All contract-type events, paginate with limit=1
    let f = serde_json::json!([{"type": "contract"}]);
    let fp = filters_param(&f);

    let resp = client
        .get(format!(
            "{}/events?ledger=100&limit=1&filters={}",
            base_url, fp
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);

    // Second page using before cursor
    let next_cursor = body["next"].as_str().unwrap();
    let resp = client
        .get(format!(
            "{}/events?limit=1&before={}&filters={}",
            base_url, next_cursor, fp
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_filters_combined_with_ledger() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // transfer events on ledger 100
    let f = serde_json::json!([{"topics": [{"symbol": "transfer"}]}]);
    let resp = client
        .get(format!(
            "{}/events?ledger=100&filters={}",
            base_url,
            filters_param(&f)
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
async fn test_filters_all_wildcards() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // All wildcards: matches events with >= 3 topics
    let f = serde_json::json!([{"topics": [null, null, null]}]);
    let resp = client
        .get(format!(
            "{}/events?ledger=100&filters={}",
            base_url,
            filters_param(&f)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Events 0 and 2 have 3 topics
    assert_eq!(data.len(), 2);
}

#[tokio::test]
async fn test_filters_empty_array() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // Empty filters = no filter constraint, returns all from ledger
    let f = serde_json::json!([]);
    let resp = client
        .get(format!(
            "{}/events?ledger=100&filters={}",
            base_url,
            filters_param(&f)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 5);
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
async fn test_filters_invalid_json() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/events?filters=not-valid-json", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("invalid filters JSON"));
}

#[tokio::test]
async fn test_filters_invalid_type() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let f = serde_json::json!([{"type": "bogus"}]);
    let resp = client
        .get(format!("{}/events?filters={}", base_url, filters_param(&f)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
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
            "ledger": 1000
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
async fn test_post_with_filters() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({
            "ledger": 100,
            "filters": [{"contract": "CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert!(data.len() > 0);
    for event in data {
        assert_eq!(
            event["contract"],
            "CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"
        );
    }
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
        .get(format!("{}/events?ledger=1000", base_url))
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
        .get(format!("{}/events?ledger=1000", base_url))
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
        .get(format!("{}/events?ledger=1000&limit=2", base_url))
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
        .get(format!("{}/events?ledger=1000", base_url))
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
        .get(format!("{}/events?ledger=1000&limit=2", base_url))
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
            "{}/events?ledger=100&q={}",
            base_url,
            q_param("type:system")
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
            "{}/events?ledger=100&q={}",
            base_url,
            q_param("contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
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
            "{}/events?ledger=100&q={}",
            base_url,
            q_param(r#"topic0:{"symbol":"transfer"}"#)
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
            "{}/events?ledger=100&q={}",
            base_url,
            q_param(r#"topic0:{"symbol":"transfer"} topic2:{"address":"GDEF"}"#)
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
    let q = r#"(contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA topic0:{"symbol":"transfer"}) OR (contract:CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB topic0:{"symbol":"transfer"})"#;
    let resp = client
        .get(format!(
            "{}/events?ledger=100&q={}",
            base_url,
            q_param(q)
        ))
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
    let q = r#"contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA type:contract topic0:{"symbol":"mint"}"#;
    let resp = client
        .get(format!(
            "{}/events?ledger=100&q={}",
            base_url,
            q_param(q)
        ))
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

    let qp = q_param("type:contract");

    // First page: limit=1
    let resp = client
        .get(format!(
            "{}/events?ledger=100&limit=1&q={}",
            base_url, qp
        ))
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
            "{}/events?ledger=100&q={}",
            base_url,
            q_param(r#"topic0:{"symbol":"transfer"}"#)
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

    let q = r#"(contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA OR contract:CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB) topic0:{"symbol":"transfer"}"#;
    let resp = client
        .get(format!(
            "{}/events?ledger=100&q={}",
            base_url,
            q_param(q)
        ))
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
        .get(format!(
            "{}/events?q={}",
            base_url,
            q_param("foo:bar")
        ))
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
            "ledger": 100,
            "q": "type:system"
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
        .get(format!("{}/events?ledger=100", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn test_q_and_filters_conflict() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let f = serde_json::json!([{"type": "system"}]);
    let resp = client
        .get(format!(
            "{}/events?q={}&filters={}",
            base_url,
            q_param("type:contract"),
            filters_param(&f)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("filters and q cannot both be provided"));
    assert_eq!(body["error"]["param"], "q");
}

#[tokio::test]
async fn test_q_and_filters_conflict_post() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/events", base_url))
        .json(&serde_json::json!({
            "q": "type:contract",
            "filters": [{"type": "system"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("filters and q cannot both be provided"));
}

#[tokio::test]
async fn test_q_filter_url_encoded() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // Manually URL-encode: type:contract topic0:{"symbol":"transfer"}
    let resp = client
        .get(format!(
            "{}/events?ledger=100&q=type%3Acontract%20topic0%3A%7B%22symbol%22%3A%22transfer%22%7D",
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
            "{}/events?ledger=100&q={}",
            base_url,
            q_param("type:diagnostic")
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
            "{}/events?ledger=100&q={}",
            base_url,
            q_param(r#"topic0:{"symbol":"transfer"} topic2:{"address":"GDDD"}"#)
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
            "{}/events?ledger=100&q={}",
            base_url,
            q_param("type:contract OR type:system OR type:diagnostic")
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
    let q = r#"contract:CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB topic0:{"symbol":"mint"}"#;
    let resp = client
        .get(format!(
            "{}/events?ledger=100&q={}",
            base_url,
            q_param(q)
        ))
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
            "{}/events?ledger=100&q={}",
            base_url,
            q_param(r#"topic:{"address":"GABC"}"#)
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
            "{}/events?ledger=100&q={}",
            base_url,
            q_param(r#"type:contract topic:{"symbol":"transfer"}"#)
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
            "{}/events?ledger=100&q={}",
            base_url,
            q_param(r#"topic:{"symbol":"transfer"} topic:{"address":"GABC"}"#)
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
            "{}/events?ledger=100&q={}",
            base_url,
            q_param(r#"topic:{"symbol":"nonexistent"}"#)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 0);
}
