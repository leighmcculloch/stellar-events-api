//! End-to-end benchmark for the cold-fetch path.
//!
//! Tests the full latency of a request that triggers on-demand backfill:
//! HTTP request → backfill → fetch from origin → zstd decompress →
//! XDR parse → event extraction → store insert → query → JSON response.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use stellar_events_api::api;
use stellar_events_api::db::{EventQueryParams, EventStore};
use stellar_events_api::ledger::events::extract_events;
use stellar_events_api::ledger::fetch::{fetch_ledger_raw, parse_ledger_batch};
use stellar_events_api::ledger::path::StoreConfig;
use stellar_events_api::AppState;

use stellar_xdr::curr::*;

/// Build a single contract event.
fn build_contract_event(contract_byte: u8, event_idx: u32) -> ContractEvent {
    let mut hash = [0u8; 32];
    hash[0] = contract_byte;

    ContractEvent {
        ext: ExtensionPoint::V0,
        contract_id: Some(ContractId(Hash(hash))),
        type_: ContractEventType::Contract,
        body: ContractEventBody::V0(ContractEventV0 {
            topics: vec![ScVal::Symbol("transfer".try_into().unwrap())]
                .try_into()
                .unwrap(),
            data: ScVal::I128(Int128Parts {
                hi: 0,
                lo: (event_idx as u64 + 1) * 1000,
            }),
        }),
    }
}

/// Build a zstd-compressed XDR LedgerCloseMetaBatch.
fn build_test_ledger_compressed(ledger_seq: u32, num_txs: usize, events_per_tx: usize) -> Vec<u8> {
    let mut tx_metas = Vec::new();

    for tx_idx in 0..num_txs {
        let mut events = Vec::new();
        for evt_idx in 0..events_per_tx {
            events.push(build_contract_event(tx_idx as u8, evt_idx as u32));
        }

        let mut tx_hash = [0u8; 32];
        tx_hash[0] = tx_idx as u8;

        let trm = TransactionResultMeta {
            result: TransactionResultPair {
                transaction_hash: Hash(tx_hash),
                result: TransactionResult {
                    fee_charged: 100,
                    result: TransactionResultResult::TxSuccess(VecM::default()),
                    ext: TransactionResultExt::V0,
                },
            },
            fee_processing: LedgerEntryChanges(VecM::default()),
            tx_apply_processing: TransactionMeta::V3(TransactionMetaV3 {
                ext: ExtensionPoint::V0,
                tx_changes_before: LedgerEntryChanges(VecM::default()),
                operations: VecM::default(),
                tx_changes_after: LedgerEntryChanges(VecM::default()),
                soroban_meta: Some(SorobanTransactionMeta {
                    ext: SorobanTransactionMetaExt::V0,
                    events: events.try_into().unwrap(),
                    return_value: ScVal::Void,
                    diagnostic_events: VecM::default(),
                }),
            }),
        };

        tx_metas.push(trm);
    }

    let header = LedgerHeader {
        ledger_version: 21,
        previous_ledger_hash: Hash([0; 32]),
        scp_value: StellarValue {
            tx_set_hash: Hash([0; 32]),
            close_time: TimePoint(1700000000),
            upgrades: VecM::default(),
            ext: StellarValueExt::Basic,
        },
        tx_set_result_hash: Hash([0; 32]),
        bucket_list_hash: Hash([0; 32]),
        ledger_seq,
        total_coins: 0,
        fee_pool: 0,
        inflation_seq: 0,
        id_pool: 0,
        base_fee: 100,
        base_reserve: 5000000,
        max_tx_set_size: 100,
        skip_list: [Hash([0; 32]), Hash([0; 32]), Hash([0; 32]), Hash([0; 32])],
        ext: LedgerHeaderExt::V0,
    };

    let meta = LedgerCloseMeta::V1(LedgerCloseMetaV1 {
        ext: LedgerCloseMetaExt::V0,
        ledger_header: LedgerHeaderHistoryEntry {
            hash: Hash([0; 32]),
            header,
            ext: LedgerHeaderHistoryEntryExt::V0,
        },
        tx_set: GeneralizedTransactionSet::V1(TransactionSetV1 {
            previous_ledger_hash: Hash([0; 32]),
            phases: VecM::default(),
        }),
        tx_processing: tx_metas.try_into().unwrap(),
        upgrades_processing: VecM::default(),
        scp_info: VecM::default(),
        total_byte_size_of_live_soroban_state: 0,
        evicted_keys: VecM::default(),
        unused: VecM::default(),
    });

    let batch = LedgerCloseMetaBatch {
        start_sequence: ledger_seq,
        end_sequence: ledger_seq,
        ledger_close_metas: vec![meta].try_into().unwrap(),
    };

    let xdr_bytes = batch.to_xdr(Limits::none()).unwrap();
    zstd::encode_all(Cursor::new(&xdr_bytes), 3).unwrap()
}

/// Start a mock S3 server that serves the given data for any path.
async fn start_mock_s3(compressed_data: Vec<u8>) -> String {
    let data = axum::body::Bytes::from(compressed_data);

    let app = axum::Router::new().fallback(move || {
        let d = data.clone();
        async move { d }
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(10)).await;
    url
}

/// Start a test server with an empty store (except for a seeded latest_ledger).
async fn start_cold_server(mock_url: &str, seed_latest: u32) -> String {
    let store = EventStore::new(24 * 60 * 60);
    store
        .record_ledger_cached(seed_latest, 0)
        .expect("failed to seed");

    let state = Arc::new(AppState {
        store,
        config: StoreConfig::default(),
        meta_url: mock_url.to_string(),
        client: reqwest::Client::new(),
    });

    let app = api::router(state, None);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(10)).await;
    base_url
}

/// End-to-end test and benchmark of the cold-fetch path.
///
/// Exercises the full latency of serving events from a ledger that hasn't
/// been cached yet, including: HTTP fetch → zstd decompress → XDR parse →
/// event extraction → store insertion → query → JSON response.
#[tokio::test]
async fn test_cold_fetch_latency() {
    // 10 transactions × 5 events = 50 events
    let compressed = build_test_ledger_compressed(1000, 10, 5);
    eprintln!("Compressed XDR size: {} bytes", compressed.len());

    let mock_url = start_mock_s3(compressed).await;
    let client = reqwest::Client::new();

    let mut durations = Vec::new();
    let iterations = 20;

    for _ in 0..iterations {
        // Fresh server each iteration to ensure cold fetch
        let base_url = start_cold_server(&mock_url, 1001).await;

        let start = std::time::Instant::now();
        let resp = client
            .get(format!("{}/events?q=ledger%3A1000&limit=100", base_url))
            .send()
            .await
            .unwrap();
        let duration = start.elapsed();

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        let data = body["data"].as_array().unwrap();
        assert_eq!(data.len(), 50, "expected 50 events (10 tx × 5 events)");
        // Verify structure
        let first = &data[0];
        assert_eq!(first["object"], "event");
        assert!(first["id"].as_str().unwrap().starts_with("evt_"));
        assert_eq!(first["ledger"], 1000);
        assert_eq!(first["type"], "contract");

        durations.push(duration);
    }

    durations.sort();

    let avg = durations.iter().map(|d| d.as_micros()).sum::<u128>() / iterations as u128;
    let p50 = durations[iterations / 2].as_micros();
    let p95 = durations[(iterations as f64 * 0.95) as usize].as_micros();
    let min = durations[0].as_micros();
    let max = durations[iterations - 1].as_micros();

    eprintln!();
    eprintln!("=== Cold Fetch Latency ({} iterations) ===", iterations);
    eprintln!("  min:  {:>8}µs ({:.2}ms)", min, min as f64 / 1000.0);
    eprintln!("  p50:  {:>8}µs ({:.2}ms)", p50, p50 as f64 / 1000.0);
    eprintln!("  avg:  {:>8}µs ({:.2}ms)", avg, avg as f64 / 1000.0);
    eprintln!("  p95:  {:>8}µs ({:.2}ms)", p95, p95 as f64 / 1000.0);
    eprintln!("  max:  {:>8}µs ({:.2}ms)", max, max as f64 / 1000.0);
    eprintln!();
}

/// Stage-by-stage breakdown of the cold-fetch pipeline.
/// Calls internal functions directly (bypassing the HTTP API layer) to isolate
/// where time is spent: fetch+decompress, XDR parse, event extraction,
/// store insert, query, and JSON serialization.
#[tokio::test]
async fn test_cold_fetch_breakdown() {
    let compressed = build_test_ledger_compressed(1000, 10, 5);
    let mock_url = start_mock_s3(compressed).await;
    let client = reqwest::Client::new();
    let config = StoreConfig::default();

    let iterations = 20;
    let mut t_fetch = Vec::new();
    let mut t_parse = Vec::new();
    let mut t_extract = Vec::new();
    let mut t_insert = Vec::new();
    let mut t_query = Vec::new();
    let mut t_json = Vec::new();

    for _ in 0..iterations {
        // Stage 1: HTTP fetch + zstd decompress
        let s = std::time::Instant::now();
        let raw = fetch_ledger_raw(&client, &mock_url, &config, 1000)
            .await
            .unwrap();
        t_fetch.push(s.elapsed());

        // Stage 2: XDR parse
        let s = std::time::Instant::now();
        let batch = parse_ledger_batch(&raw).unwrap();
        t_parse.push(s.elapsed());

        // Stage 3: Event extraction
        let s = std::time::Instant::now();
        let events = extract_events(&batch);
        t_extract.push(s.elapsed());

        assert_eq!(events.len(), 50);

        // Stage 4: Store insertion
        let store = EventStore::new(24 * 60 * 60);
        let s = std::time::Instant::now();
        store.insert_events(events).unwrap();
        t_insert.push(s.elapsed());

        // Stage 5: Query
        let params = EventQueryParams {
            limit: 100,
            after: None,
            before: None,
            filters: vec![stellar_events_api::db::EventFilter {
                ledger: Some(1000),
                ..Default::default()
            }],
        };
        let s = std::time::Instant::now();
        let result = store.query_events(&params).unwrap();
        t_query.push(s.elapsed());

        assert_eq!(result.data.len(), 50);

        // Stage 6: Event conversion + JSON serialization
        let s = std::time::Instant::now();
        let events: Vec<stellar_events_api::api::types::Event> =
            result.data.into_iter().map(|r| r.into()).collect();
        let _json = serde_json::to_string(&events).unwrap();
        t_json.push(s.elapsed());
    }

    fn p50_us(v: &mut [Duration]) -> u128 {
        v.sort();
        v[v.len() / 2].as_micros()
    }

    let total = p50_us(&mut t_fetch)
        + p50_us(&mut t_parse)
        + p50_us(&mut t_extract)
        + p50_us(&mut t_insert)
        + p50_us(&mut t_query)
        + p50_us(&mut t_json);

    eprintln!();
    eprintln!(
        "=== Cold Fetch Breakdown (p50, {} iterations) ===",
        iterations
    );
    eprintln!(
        "  fetch+decompress: {:>6}µs ({:.0}%)",
        p50_us(&mut t_fetch),
        p50_us(&mut t_fetch) as f64 / total as f64 * 100.0
    );
    eprintln!(
        "  XDR parse:        {:>6}µs ({:.0}%)",
        p50_us(&mut t_parse),
        p50_us(&mut t_parse) as f64 / total as f64 * 100.0
    );
    eprintln!(
        "  event extraction: {:>6}µs ({:.0}%)",
        p50_us(&mut t_extract),
        p50_us(&mut t_extract) as f64 / total as f64 * 100.0
    );
    eprintln!(
        "  store insert:     {:>6}µs ({:.0}%)",
        p50_us(&mut t_insert),
        p50_us(&mut t_insert) as f64 / total as f64 * 100.0
    );
    eprintln!(
        "  query:            {:>6}µs ({:.0}%)",
        p50_us(&mut t_query),
        p50_us(&mut t_query) as f64 / total as f64 * 100.0
    );
    eprintln!(
        "  JSON serialize:   {:>6}µs ({:.0}%)",
        p50_us(&mut t_json),
        p50_us(&mut t_json) as f64 / total as f64 * 100.0
    );
    eprintln!("  TOTAL:            {:>6}µs", total);
    eprintln!();
}
