use std::sync::Arc;
use std::time::Duration;

use crate::ledger::events::{extract_events, ExtractedEvent};
use crate::ledger::fetch::{fetch_ledger_raw, parse_ledger_batch};
use crate::ledger::path::StoreConfig;
use crate::AppState;

/// How often to poll for new ledgers.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// How often to run the cleanup task.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(3600);

/// Background sync task that proactively fetches new ledgers.
pub async fn run_sync(
    client: reqwest::Client,
    meta_url: String,
    store_config: StoreConfig,
    state: Arc<AppState>,
    start_ledger: Option<u32>,
    parallel_fetches: u32,
) {
    // Determine starting point
    let mut current_ledger = match start_ledger {
        Some(seq) => seq,
        None => {
            // Try to resume from where we left off
            let last = state
                .store
                .get_sync_state("last_synced_ledger")
                .ok()
                .flatten()
                .and_then(|v| v.parse::<u32>().ok());
            match last {
                Some(seq) => seq + 1,
                None => {
                    // Try to discover the latest ledger from Horizon
                    match discover_latest_ledger(&client).await {
                        Some(seq) => {
                            tracing::info!(ledger = seq, "discovered latest ledger from horizon");
                            // Start a few ledgers back to have some initial data
                            seq.saturating_sub(10)
                        }
                        None => {
                            tracing::warn!(
                                "could not discover latest ledger, starting from a recent default"
                            );
                            // Fallback: a recent known ledger
                            58_000_000
                        }
                    }
                }
            }
        }
    };

    tracing::info!(start = current_ledger, "starting ledger sync");

    // Spawn cleanup task
    let cleanup_state = Arc::clone(&state);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(CLEANUP_INTERVAL).await;
            match cleanup_state.store.cleanup_expired() {
                Ok(count) if count > 0 => {
                    tracing::info!(count, "cleaned up expired ledger cache entries");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "error during cleanup");
                }
                _ => {}
            }
        }
    });

    let mut consecutive_failures = 0u32;

    loop {
        // Skip any cached ledgers
        loop {
            let cached = state
                .store
                .is_ledger_cached(current_ledger)
                .unwrap_or(false);
            if cached {
                current_ledger += 1;
                consecutive_failures = 0;
            } else {
                break;
            }
        }

        // Build batch of ledger sequences to fetch
        let batch_sequences: Vec<u32> =
            (current_ledger..current_ledger + parallel_fetches).collect();

        // Launch all fetches concurrently
        let futures: Vec<_> = batch_sequences
            .iter()
            .map(|&seq| fetch_and_extract(&client, &meta_url, &store_config, seq))
            .collect();
        let results = futures::future::join_all(futures).await;

        // Process results in strict ledger order
        let mut advanced = 0u32;
        let mut total_events = 0usize;
        let mut should_sleep = None;

        for (i, result) in results.into_iter().enumerate() {
            let seq = batch_sequences[i];
            match result {
                Ok(events) => {
                    let event_count = events.len();
                    let db_result = (|| -> Result<(), crate::Error> {
                        state.store.insert_events(&events)?;
                        state.store.record_ledger_cached(seq, 0)?;
                        state
                            .store
                            .set_sync_state("last_synced_ledger", &seq.to_string())?;
                        Ok(())
                    })();

                    if let Err(e) = db_result {
                        tracing::warn!(ledger = seq, error = %e, "failed to store ledger events");
                        should_sleep = Some(SleepReason::Error);
                        break;
                    }

                    advanced += 1;
                    total_events += event_count;
                    consecutive_failures = 0;
                }
                Err(e) if matches!(e, crate::Error::LedgerNotFound(_)) => {
                    tracing::debug!(ledger = seq, "ledger not yet available, waiting");
                    should_sleep = Some(SleepReason::NotFound);
                    break;
                }
                Err(e) => {
                    consecutive_failures += 1;
                    tracing::warn!(
                        ledger = seq,
                        error = %e,
                        consecutive_failures,
                        "failed to fetch ledger"
                    );
                    should_sleep = Some(SleepReason::Error);
                    break;
                }
            }
        }

        if advanced > 0 {
            let start = current_ledger;
            let end = current_ledger + advanced - 1;
            tracing::info!(
                ledgers = format!("{}..{}", start, end),
                events = total_events,
                "synced ledgers"
            );
            current_ledger += advanced;

            // Periodically update query planner statistics (no-op for in-memory store)
            if current_ledger % 1000 < parallel_fetches {
                let _ = state.store.analyze();
            }
        }

        match should_sleep {
            Some(SleepReason::NotFound) => {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            Some(SleepReason::Error) => {
                let backoff =
                    Duration::from_secs((2u64.pow(consecutive_failures.min(6))).min(60));
                tokio::time::sleep(backoff).await;
            }
            None => {
                // Entire batch succeeded, immediately continue
            }
        }
    }
}

enum SleepReason {
    NotFound,
    Error,
}

/// Fetch a ledger, decompress, parse, and extract events (no DB access).
#[tracing::instrument(skip(client, meta_url, store_config))]
pub async fn fetch_and_extract(
    client: &reqwest::Client,
    meta_url: &str,
    store_config: &StoreConfig,
    ledger_sequence: u32,
) -> Result<Vec<ExtractedEvent>, crate::Error> {
    let raw = fetch_ledger_raw(client, meta_url, store_config, ledger_sequence).await?;
    let batch = parse_ledger_batch(&raw)?;
    let events = extract_events(&batch);
    tracing::trace!(ledger = ledger_sequence, events = events.len(), "extracted events");
    Ok(events)
}

/// Try to discover the latest ledger sequence from Horizon.
async fn discover_latest_ledger(client: &reqwest::Client) -> Option<u32> {
    let resp = client
        .get("https://horizon.stellar.org/")
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let body: serde_json::Value = serde_json::from_slice(&resp.bytes().await.ok()?).ok()?;
    body.get("history_latest_ledger")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
}
