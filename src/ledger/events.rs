use stellar_xdr::curr::{
    ContractEvent, ContractEventType, LedgerCloseMeta, LedgerCloseMetaBatch, TransactionMeta,
    TransactionMetaV3, TransactionMetaV4,
};

pub use super::event_id::EventPhase;

/// A structured event extracted from ledger close meta.
#[derive(Debug, Clone)]
pub struct ExtractedEvent {
    pub ledger_sequence: u32,
    pub ledger_closed_at: i64,
    pub phase: EventPhase,
    pub tx_index: u32,
    pub event_index: u32,
    pub tx_hash: String,
    pub contract_id: Option<String>,
    pub event_type: EventType,
    pub topics_xdr_json: Vec<serde_json::Value>,
    pub data_xdr_json: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    Contract,
    System,
    Diagnostic,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventType::Contract => write!(f, "contract"),
            EventType::System => write!(f, "system"),
            EventType::Diagnostic => write!(f, "diagnostic"),
        }
    }
}

impl std::str::FromStr for EventType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "contract" => Ok(EventType::Contract),
            "system" => Ok(EventType::System),
            "diagnostic" => Ok(EventType::Diagnostic),
            _ => Err(format!("unknown event type: {}", s)),
        }
    }
}

impl From<ContractEventType> for EventType {
    fn from(t: ContractEventType) -> Self {
        match t {
            ContractEventType::System => EventType::System,
            ContractEventType::Contract => EventType::Contract,
            ContractEventType::Diagnostic => EventType::Diagnostic,
        }
    }
}

/// Extract the ledger close time from a LedgerCloseMeta.
fn ledger_close_time(meta: &LedgerCloseMeta) -> i64 {
    match meta {
        LedgerCloseMeta::V0(v0) => v0.ledger_header.header.scp_value.close_time.0 as i64,
        LedgerCloseMeta::V1(v1) => v1.ledger_header.header.scp_value.close_time.0 as i64,
        LedgerCloseMeta::V2(v2) => v2.ledger_header.header.scp_value.close_time.0 as i64,
    }
}

/// Extract the ledger sequence from a LedgerCloseMeta.
fn ledger_sequence_num(meta: &LedgerCloseMeta) -> u32 {
    match meta {
        LedgerCloseMeta::V0(v0) => v0.ledger_header.header.ledger_seq,
        LedgerCloseMeta::V1(v1) => v1.ledger_header.header.ledger_seq,
        LedgerCloseMeta::V2(v2) => v2.ledger_header.header.ledger_seq,
    }
}

/// Extract contract events from a single ContractEvent XDR.
fn extract_contract_event(
    event: &ContractEvent,
    id_cache: &mut ContractIdCache,
) -> (
    Option<String>,
    EventType,
    Vec<serde_json::Value>,
    serde_json::Value,
) {
    let contract_id = event
        .contract_id
        .as_ref()
        .map(|id| id_cache.get_or_insert(&id.0));
    let event_type = EventType::from(event.type_);

    let (topics, data) = match &event.body {
        stellar_xdr::curr::ContractEventBody::V0(v0) => {
            let topics: Vec<serde_json::Value> = v0
                .topics
                .iter()
                .map(|t| serde_json::to_value(t).unwrap_or(serde_json::Value::Null))
                .collect();
            let data = serde_json::to_value(&v0.data).unwrap_or(serde_json::Value::Null);
            (topics, data)
        }
    };

    (contract_id, event_type, topics, data)
}

/// Convert a contract ID hash to Stellar strkey format (C...).
fn contract_strkey(hash: &stellar_xdr::curr::Hash) -> String {
    stellar_strkey::Contract(hash.0).to_string()
}

/// Extract events from a single transaction's `TransactionMeta`.
fn extract_events_from_tx_meta(
    tx_meta: &TransactionMeta,
    seq: u32,
    close_time: i64,
    tx_idx: u32,
    tx_hash: &str,
    events: &mut Vec<ExtractedEvent>,
    id_cache: &mut ContractIdCache,
) {
    match tx_meta {
        TransactionMeta::V3(v3) => {
            extract_events_from_v3(v3, seq, close_time, tx_idx, tx_hash, events, id_cache);
        }
        TransactionMeta::V4(v4) => {
            extract_events_from_v4(v4, seq, close_time, tx_idx, tx_hash, events, id_cache);
        }
        _ => {}
    }
}

/// Extract events from TransactionMetaV3 (Protocol 20-21).
/// V3 has no stages or per-operation events; all events are operation-level.
fn extract_events_from_v3(
    v3: &TransactionMetaV3,
    seq: u32,
    close_time: i64,
    tx_idx: u32,
    tx_hash: &str,
    events: &mut Vec<ExtractedEvent>,
    id_cache: &mut ContractIdCache,
) {
    if let Some(soroban) = &v3.soroban_meta {
        for (evt_idx, contract_event) in soroban.events.iter().enumerate() {
            push_event(
                contract_event,
                seq,
                close_time,
                EventPhase::Operation,
                tx_idx,
                evt_idx as u32,
                tx_hash,
                events,
                id_cache,
            );
        }
    }
}

/// Extract events from TransactionMetaV4 (Protocol 22+).
/// Events come from two sources:
///   - v4.operations[i].events: per-operation contract events (Operation phase)
///   - v4.events: transaction-level events with stages (mapped to EventPhase)
fn extract_events_from_v4(
    v4: &TransactionMetaV4,
    seq: u32,
    close_time: i64,
    tx_idx: u32,
    tx_hash: &str,
    events: &mut Vec<ExtractedEvent>,
    id_cache: &mut ContractIdCache,
) {
    // Operation-level events, flattened across all operations
    let mut op_evt_idx: u32 = 0;
    for op_meta in v4.operations.iter() {
        for contract_event in op_meta.events.iter() {
            push_event(
                contract_event,
                seq,
                close_time,
                EventPhase::Operation,
                tx_idx,
                op_evt_idx,
                tx_hash,
                events,
                id_cache,
            );
            op_evt_idx += 1;
        }
    }

    // Transaction-level events (stage determines phase)
    for (evt_idx, tx_event) in v4.events.iter().enumerate() {
        let phase = match tx_event.stage {
            stellar_xdr::curr::TransactionEventStage::BeforeAllTxs => EventPhase::BeforeAllTxs,
            stellar_xdr::curr::TransactionEventStage::AfterTx => EventPhase::AfterTx,
            stellar_xdr::curr::TransactionEventStage::AfterAllTxs => EventPhase::AfterAllTxs,
        };
        push_event(
            &tx_event.event,
            seq,
            close_time,
            phase,
            tx_idx,
            evt_idx as u32,
            tx_hash,
            events,
            id_cache,
        );
    }
}

/// Build an ExtractedEvent from a ContractEvent and push it.
fn push_event(
    contract_event: &ContractEvent,
    seq: u32,
    close_time: i64,
    phase: EventPhase,
    tx_idx: u32,
    evt_idx: u32,
    tx_hash: &str,
    events: &mut Vec<ExtractedEvent>,
    id_cache: &mut ContractIdCache,
) {
    let (contract_id, event_type, topics, data) = extract_contract_event(contract_event, id_cache);

    events.push(ExtractedEvent {
        ledger_sequence: seq,
        ledger_closed_at: close_time,
        phase,
        tx_index: tx_idx,
        event_index: evt_idx,
        tx_hash: tx_hash.to_string(),
        contract_id,
        event_type,
        topics_xdr_json: topics,
        data_xdr_json: data,
    });
}

/// Cache for contract ID strkey encoding, avoiding redundant conversions
/// for events from the same contract within a batch.
struct ContractIdCache {
    cache: std::collections::HashMap<[u8; 32], String>,
}

impl ContractIdCache {
    fn new() -> Self {
        Self {
            cache: std::collections::HashMap::new(),
        }
    }

    fn get_or_insert(&mut self, hash: &stellar_xdr::curr::Hash) -> String {
        self.cache
            .entry(hash.0)
            .or_insert_with(|| contract_strkey(hash))
            .clone()
    }
}

/// Extract all events from a LedgerCloseMetaBatch.
pub fn extract_events(batch: &LedgerCloseMetaBatch) -> Vec<ExtractedEvent> {
    // Pre-allocate based on number of transactions (heuristic: ~5 events per tx).
    let tx_count: usize = batch
        .ledger_close_metas
        .iter()
        .map(|m| match m {
            LedgerCloseMeta::V0(v0) => v0.tx_processing.len(),
            LedgerCloseMeta::V1(v1) => v1.tx_processing.len(),
            LedgerCloseMeta::V2(v2) => v2.tx_processing.len(),
        })
        .sum();
    let mut events = Vec::with_capacity(tx_count * 5);
    let mut id_cache = ContractIdCache::new();

    for ledger_meta in batch.ledger_close_metas.iter() {
        let seq = ledger_sequence_num(ledger_meta);
        let close_time = ledger_close_time(ledger_meta);

        match ledger_meta {
            LedgerCloseMeta::V0(v0) => {
                for (tx_idx, trm) in v0.tx_processing.iter().enumerate() {
                    let tx_hash = hex::encode(v0.tx_set.previous_ledger_hash.0);
                    extract_events_from_tx_meta(
                        &trm.tx_apply_processing,
                        seq,
                        close_time,
                        tx_idx as u32,
                        &tx_hash,
                        &mut events,
                        &mut id_cache,
                    );
                }
            }
            LedgerCloseMeta::V1(v1) => {
                for (tx_idx, trm) in v1.tx_processing.iter().enumerate() {
                    let tx_hash = hex::encode(trm.result.transaction_hash.0);
                    extract_events_from_tx_meta(
                        &trm.tx_apply_processing,
                        seq,
                        close_time,
                        tx_idx as u32,
                        &tx_hash,
                        &mut events,
                        &mut id_cache,
                    );
                }
            }
            LedgerCloseMeta::V2(v2) => {
                for (tx_idx, trm) in v2.tx_processing.iter().enumerate() {
                    let tx_hash = hex::encode(trm.result.transaction_hash.0);
                    extract_events_from_tx_meta(
                        &trm.tx_apply_processing,
                        seq,
                        close_time,
                        tx_idx as u32,
                        &tx_hash,
                        &mut events,
                        &mut id_cache,
                    );
                }
            }
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_type_display() {
        assert_eq!(EventType::Contract.to_string(), "contract");
        assert_eq!(EventType::System.to_string(), "system");
        assert_eq!(EventType::Diagnostic.to_string(), "diagnostic");
    }
}
