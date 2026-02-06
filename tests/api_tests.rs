use std::sync::{Arc, Mutex};
use std::time::Duration;

use stellar_events_api::api;
use stellar_events_api::db::EventStore;
use stellar_events_api::ledger::events::{EventPhase, EventType, ExtractedEvent};
use stellar_events_api::ledger::path::StoreConfig;
use stellar_events_api::AppState;

/// Helper: start a test server and return its base URL.
async fn start_test_server(events: Vec<ExtractedEvent>) -> String {
    let db = EventStore::open_memory().expect("failed to open in-memory db");
    if !events.is_empty() {
        db.insert_events(&events).expect("failed to insert events");
    }

    let state = Arc::new(AppState {
        db: Mutex::new(db),
        config: StoreConfig::default(),
        meta_url: String::new(),
        client: reqwest::Client::new(),
    });

    let app = api::router(state);

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

/// Helper: create test events with simple topics.
fn make_test_events(count: usize, start_ledger: u32) -> Vec<ExtractedEvent> {
    (0..count)
        .map(|i| ExtractedEvent {
            ledger_sequence: start_ledger + i as u32,
            ledger_closed_at: 1700000000 + (i as i64 * 5),
            phase: EventPhase::Operation,
            tx_index: 0,
            event_index: 0,
            tx_hash: format!("{:064x}", i),
            contract_id: Some(format!(
                "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA{}",
                i
            )),
            event_type: EventType::Contract,
            topics_xdr_json: vec![serde_json::json!({"symbol": "transfer"})],
            data_xdr_json: serde_json::json!({"amount": i * 100}),
            event_xdr_json: serde_json::json!({"type": "contract", "body": {}}),
        })
        .collect()
}

/// Create events with varied types, contracts, and XDR-JSON ScVal topics.
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
            event_xdr_json: serde_json::json!({"type": "contract"}),
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
            event_xdr_json: serde_json::json!({"type": "system"}),
        },
        // Event 2: transfer on contract CB
        ExtractedEvent {
            ledger_sequence: 101,
            ledger_closed_at: 1700000005,
            phase: EventPhase::Operation,
            tx_index: 0,
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
            event_xdr_json: serde_json::json!({"type": "contract"}),
        },
        // Event 3: mint on contract CA
        ExtractedEvent {
            ledger_sequence: 102,
            ledger_closed_at: 1700000010,
            phase: EventPhase::Operation,
            tx_index: 0,
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
            event_xdr_json: serde_json::json!({"type": "contract"}),
        },
        // Event 4: diagnostic on contract CA
        ExtractedEvent {
            ledger_sequence: 102,
            ledger_closed_at: 1700000010,
            phase: EventPhase::Operation,
            tx_index: 0,
            event_index: 1,
            tx_hash: "c".repeat(64),
            contract_id: Some(
                "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
            ),
            event_type: EventType::Diagnostic,
            topics_xdr_json: vec![serde_json::json!({"symbol": "diag"})],
            data_xdr_json: serde_json::json!({}),
            event_xdr_json: serde_json::json!({"type": "diagnostic"}),
        },
    ]
}

fn filters_param(filters: &serde_json::Value) -> String {
    urlencoding::encode(&filters.to_string()).to_string()
}

// --- Basic list and pagination ---

#[tokio::test]
async fn test_list_events_empty() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/v1/events", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "list");
    assert_eq!(body["url"], "/v1/events");
    assert_eq!(body["has_more"], false);
    assert!(body["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_list_events_with_data() {
    let events = make_test_events(5, 1000);
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/v1/events?start_ledger=1000&limit=3", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "list");
    assert_eq!(body["has_more"], true);
    assert_eq!(body["data"].as_array().unwrap().len(), 3);

    let first = &body["data"][0];
    assert_eq!(first["object"], "event");
    assert!(first["id"].as_str().unwrap().starts_with("evt_"));
    assert!(first["ledger_sequence"].is_number());
    assert!(first["ledger_closed_at"].is_string());
    assert!(first["tx_hash"].is_string());
    assert!(first["type"].is_string());
}

#[tokio::test]
async fn test_pagination_forward() {
    let events = make_test_events(5, 1000);
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // First page
    let resp = client
        .get(format!("{}/v1/events?start_ledger=1000&limit=2", base_url))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["has_more"], true);
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    let last_id = data[1]["id"].as_str().unwrap().to_string();

    // Second page
    let resp = client
        .get(format!(
            "{}/v1/events?limit=2&start_after={}",
            base_url, last_id
        ))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["has_more"], true);
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    assert!(data[0]["id"].as_str().unwrap() > last_id.as_str());

    // Third page (last)
    let last_id = data[1]["id"].as_str().unwrap().to_string();
    let resp = client
        .get(format!(
            "{}/v1/events?limit=2&start_after={}",
            base_url, last_id
        ))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["has_more"], false);
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_default_limit() {
    let events = make_test_events(15, 1000);
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/v1/events?start_ledger=1000", base_url))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 10);
    assert_eq!(body["has_more"], true);
}

#[tokio::test]
async fn test_default_starts_at_latest_ledger() {
    let events = make_test_events(5, 1000);
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // No start_ledger or start_after — should default to latest ledger (1004)
    let resp = client
        .get(format!("{}/v1/events", base_url))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["ledger_sequence"], 1004);
    assert_eq!(body["has_more"], false);
}

// --- Ledger sequence filter ---

#[tokio::test]
async fn test_start_ledger() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // start_ledger=101 returns events from ledger 101 onward (101 + 102 = 3 events)
    let resp = client
        .get(format!(
            "{}/v1/events?start_ledger=101",
            base_url
        ))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 3);
    for evt in data {
        assert!(evt["ledger_sequence"].as_u64().unwrap() >= 101);
    }
}

#[tokio::test]
async fn test_start_ledger_from_beginning() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // start_ledger=100 returns all events (ledgers 100, 101, 102)
    let resp = client
        .get(format!(
            "{}/v1/events?start_ledger=100",
            base_url
        ))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 5);
}

// --- Structured filters ---

#[tokio::test]
async fn test_filter_by_contract_id() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let f = serde_json::json!([{
        "contract_id": "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
    }]);
    let resp = client
        .get(format!(
            "{}/v1/events?start_ledger=100&filters={}",
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
            evt["contract_id"],
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
            "{}/v1/events?start_ledger=100&filters={}",
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
    let f = serde_json::json!([{"tx_hash": tx_hash}]);
    let resp = client
        .get(format!(
            "{}/v1/events?start_ledger=100&filters={}",
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
    assert_eq!(data[0]["tx_hash"], tx_hash);
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
            "{}/v1/events?start_ledger=100&filters={}",
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
    let f = serde_json::json!([{"topics": [{"symbol": "transfer"}, "*", {"address": "GDEF"}]}]);
    let resp = client
        .get(format!(
            "{}/v1/events?start_ledger=100&filters={}",
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
        serde_json::json!([{"topics": [{"symbol": "transfer"}, "*", "*", {"symbol": "extra"}]}]);
    let resp = client
        .get(format!(
            "{}/v1/events?start_ledger=100&filters={}",
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
            "contract_id": "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "topics": [{"symbol": "transfer"}]
        },
        {
            "contract_id": "CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
            "topics": [{"symbol": "transfer"}]
        }
    ]);
    let resp = client
        .get(format!(
            "{}/v1/events?start_ledger=100&filters={}",
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
        "contract_id": "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "type": "contract",
        "topics": [{"symbol": "mint"}]
    }]);
    let resp = client
        .get(format!(
            "{}/v1/events?start_ledger=100&filters={}",
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
    assert_eq!(data[0]["ledger_sequence"], 102);
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
            "{}/v1/events?start_ledger=100&limit=1&filters={}",
            base_url, fp
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(body["has_more"], true);

    // Second page
    let last_id = data[0]["id"].as_str().unwrap();
    let resp = client
        .get(format!(
            "{}/v1/events?limit=1&start_after={}&filters={}",
            base_url, last_id, fp
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["has_more"], true);
}

#[tokio::test]
async fn test_filters_combined_with_start_ledger() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // transfer events starting from ledger 101
    let f = serde_json::json!([{"topics": [{"symbol": "transfer"}]}]);
    let resp = client
        .get(format!(
            "{}/v1/events?start_ledger=101&filters={}",
            base_url,
            filters_param(&f)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    // Only event 2 (ledger 101) has transfer topic at or after ledger 101
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["ledger_sequence"], 101);
}

#[tokio::test]
async fn test_filters_all_wildcards() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    // All wildcards: matches events with >= 3 topics
    let f = serde_json::json!([{"topics": ["*", "*", "*"]}]);
    let resp = client
        .get(format!(
            "{}/v1/events?start_ledger=100&filters={}",
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

    // Empty filters = no filter constraint, returns all from start_ledger
    let f = serde_json::json!([]);
    let resp = client
        .get(format!(
            "{}/v1/events?start_ledger=100&filters={}",
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
        .get(format!("{}/v1/events?limit=0", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let resp = client
        .get(format!("{}/v1/events?limit=101", base_url))
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
        .get(format!(
            "{}/v1/events?start_after=invalid_cursor",
            base_url
        ))
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
        .get(format!("{}/v1/events?filters=not-valid-json", base_url))
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
        .get(format!(
            "{}/v1/events?filters={}",
            base_url,
            filters_param(&f)
        ))
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
        .get(format!("{}/v1/events?limit=abc", base_url))
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
        .post(format!("{}/v1/events", base_url))
        .json(&serde_json::json!({
            "limit": 2,
            "start_ledger": 1000
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "list");
    assert_eq!(body["has_more"], true);
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_post_with_filters() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/events", base_url))
        .json(&serde_json::json!({
            "start_ledger": 100,
            "filters": [{"contract_id": "CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"}]
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
            event["contract_id"],
            "CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"
        );
    }
}

#[tokio::test]
async fn test_post_invalid_limit() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/events", base_url))
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
        .post(format!("{}/v1/events", base_url))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "list");
}

// --- Response shape ---

#[tokio::test]
async fn test_stripe_list_envelope_consistency() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/v1/events", base_url))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();

    assert!(body.get("object").is_some());
    assert!(body.get("url").is_some());
    assert!(body.get("has_more").is_some());
    assert!(body.get("data").is_some());
    assert_eq!(body.as_object().unwrap().len(), 4);
}

#[tokio::test]
async fn test_event_fields_complete() {
    let events = make_multi_type_events();
    let base_url = start_test_server(events).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/v1/events?limit=1", base_url))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let event = &body["data"][0];

    assert!(event["id"].is_string());
    assert_eq!(event["object"], "event");
    assert!(event["type"].is_string());
    assert!(event["ledger_sequence"].is_number());
    assert!(event["ledger_closed_at"].is_string());
    assert!(event["tx_hash"].is_string());
    assert!(!event["topics"].is_null());
    assert!(!event["data"].is_null());
}

#[tokio::test]
async fn test_status_endpoint() {
    let base_url = start_test_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/v1/health", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["network_passphrase"].is_string());
    assert!(body["version"].is_string());
}
