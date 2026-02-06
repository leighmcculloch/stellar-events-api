use std::io::{Cursor, Read};

use stellar_xdr::curr::{LedgerCloseMetaBatch, Limits, ReadXdr};

use super::path::StoreConfig;
use crate::Error;

/// Fetches the store configuration from the remote endpoint.
pub async fn fetch_config(client: &reqwest::Client, meta_url: &str) -> Result<StoreConfig, Error> {
    let url = format!("{}/.config.json", meta_url);
    tracing::info!(url = %url, "fetching store config");
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(Error::ConfigNotFound(url));
    }
    let bytes = resp.bytes().await?;
    let config: StoreConfig = serde_json::from_slice(&bytes)?;
    tracing::info!(
        ledgers_per_batch = config.ledgers_per_batch,
        batches_per_partition = config.batches_per_partition,
        "store config loaded"
    );
    Ok(config)
}

/// Fetches and parses a ledger close meta batch for the given ledger sequence.
/// Returns the decompressed XDR bytes.
pub async fn fetch_ledger_raw(
    client: &reqwest::Client,
    meta_url: &str,
    config: &StoreConfig,
    ledger_sequence: u32,
) -> Result<Vec<u8>, Error> {
    let path = config.path_for_ledger(ledger_sequence);
    let url = format!("{}/{}", meta_url, path);
    tracing::debug!(url = %url, ledger = ledger_sequence, "fetching ledger");

    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(Error::LedgerNotFound(ledger_sequence));
    }

    let compressed = resp.bytes().await?;
    let mut decoder = zstd::stream::Decoder::new(Cursor::new(compressed))?;
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;

    Ok(decompressed)
}

/// Parse decompressed XDR bytes into a LedgerCloseMetaBatch.
pub fn parse_ledger_batch(data: &[u8]) -> Result<LedgerCloseMetaBatch, Error> {
    let cursor = Cursor::new(data);
    let mut limited = stellar_xdr::curr::Limited::new(cursor, Limits::none());
    let batch = LedgerCloseMetaBatch::read_xdr(&mut limited)?;
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_batch_fails() {
        let result = parse_ledger_batch(&[]);
        assert!(result.is_err());
    }
}
