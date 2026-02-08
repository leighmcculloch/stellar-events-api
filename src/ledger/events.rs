use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use stellar_xdr::curr::{
    ContractEvent, ContractEventType, LedgerCloseMeta, LedgerCloseMetaBatch, TransactionMeta,
    TransactionMetaV3, TransactionMetaV4,
};

/// Execution phase of an event within a ledger, encoding execution order.
/// The (phase, sub) values produce correct lexicographic ordering in event IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EventPhase {
    /// Events emitted before any transaction is applied (phase=0, sub=0).
    BeforeAllTxs,
    /// Events emitted during operation execution (phase=1, sub=0).
    Operation,
    /// Events emitted after a transaction's operations complete (phase=1, sub=1).
    AfterTx,
    /// Events emitted after all transactions are applied (phase=2, sub=0).
    AfterAllTxs,
}

impl EventPhase {
    /// Encode as (phase, sub) for use in event IDs.
    pub fn as_phase_sub(&self) -> (u8, u8) {
        match self {
            EventPhase::BeforeAllTxs => (0, 0),
            EventPhase::Operation => (1, 0),
            EventPhase::AfterTx => (1, 1),
            EventPhase::AfterAllTxs => (2, 0),
        }
    }
}

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

/// Build a deterministic event ID that encodes execution order.
/// Format: evt_{ledger}_{phase}_{tx}_{sub}_{event}
///
/// Ordering within a ledger:
///   phase 0, sub 0: BeforeAllTxs events
///   phase 1, sub 0: operation events (per tx, ordered by tx_index)
///   phase 1, sub 1: after-tx events (per tx, ordered by tx_index)
///   phase 2, sub 0: AfterAllTxs events
pub fn event_id(
    ledger_sequence: u32,
    event_phase: EventPhase,
    tx_index: u32,
    event_index: u32,
) -> String {
    let (phase, sub) = event_phase.as_phase_sub();
    format!(
        "evt_{:010}_{:01}_{:04}_{:01}_{:04}",
        ledger_sequence, phase, tx_index, sub, event_index
    )
}

/// Parse an internal event ID back into its components.
pub fn parse_event_id(id: &str) -> Option<(u32, u8, u32, u8, u32)> {
    let parts: Vec<&str> = id.strip_prefix("evt_")?.split('_').collect();
    if parts.len() != 5 {
        return None;
    }
    let ledger_sequence: u32 = parts[0].parse().ok()?;
    let phase: u8 = parts[1].parse().ok()?;
    let tx: u32 = parts[2].parse().ok()?;
    let sub: u8 = parts[3].parse().ok()?;
    let event: u32 = parts[4].parse().ok()?;
    Some((ledger_sequence, phase, tx, sub, event))
}

/// Fixed XOR key for obfuscating event IDs.
const XOR_KEY: [u8; 14] = [
    0xa3, 0x7b, 0x1c, 0xf0, 0x5e, 0xd2, 0x94, 0x68, 0x0b, 0xe7, 0x3f, 0x81, 0xc6, 0x4d,
];

/// Encode event ID components into an opaque external format.
///
/// Packs 5 components into 14 bytes big-endian, XORs with a fixed key,
/// then base64url encodes (no padding) with an `evt_` prefix.
pub fn encode_event_id(
    ledger_sequence: u32,
    phase: u8,
    tx_index: u32,
    sub: u8,
    event_index: u32,
) -> String {
    let mut buf = [0u8; 14];
    buf[0..4].copy_from_slice(&ledger_sequence.to_be_bytes());
    buf[4] = phase;
    buf[5..9].copy_from_slice(&tx_index.to_be_bytes());
    buf[9] = sub;
    buf[10..14].copy_from_slice(&event_index.to_be_bytes());

    for i in 0..14 {
        buf[i] ^= XOR_KEY[i];
    }

    let encoded = URL_SAFE_NO_PAD.encode(buf);
    format!("evt_{}", encoded)
}

/// Decode an opaque external event ID back into its components.
pub fn decode_event_id(id: &str) -> Option<(u32, u8, u32, u8, u32)> {
    let payload = id.strip_prefix("evt_")?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    if bytes.len() != 14 {
        return None;
    }

    let mut buf = [0u8; 14];
    buf.copy_from_slice(&bytes);
    for i in 0..14 {
        buf[i] ^= XOR_KEY[i];
    }

    let ledger_sequence = u32::from_be_bytes(buf[0..4].try_into().ok()?);
    let phase = buf[4];
    let tx_index = u32::from_be_bytes(buf[5..9].try_into().ok()?);
    let sub = buf[9];
    let event_index = u32::from_be_bytes(buf[10..14].try_into().ok()?);

    // Validate ranges
    if phase > 2 || sub > 1 {
        return None;
    }

    Some((ledger_sequence, phase, tx_index, sub, event_index))
}

/// Convert an internal event ID to an opaque external ID.
pub fn to_external_id(internal_id: &str) -> Option<String> {
    let (ledger, phase, tx, sub, event) = parse_event_id(internal_id)?;
    Some(encode_event_id(ledger, phase, tx, sub, event))
}

/// Convert an opaque external event ID to an internal ID.
pub fn to_internal_id(external_id: &str) -> Option<String> {
    let (ledger, phase, tx, sub, event) = decode_event_id(external_id)?;
    let event_phase = match (phase, sub) {
        (0, 0) => EventPhase::BeforeAllTxs,
        (1, 0) => EventPhase::Operation,
        (1, 1) => EventPhase::AfterTx,
        (2, 0) => EventPhase::AfterAllTxs,
        _ => return None,
    };
    Some(event_id(ledger, event_phase, tx, event))
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
) -> (
    Option<String>,
    EventType,
    Vec<serde_json::Value>,
    serde_json::Value,
) {
    let contract_id = event.contract_id.as_ref().map(|id| stellar_strkey(&id.0));
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
fn stellar_strkey(hash: &stellar_xdr::curr::Hash) -> String {
    // Contract IDs use the 'C' prefix in strkey encoding.
    // The strkey format is: version_byte + payload + checksum
    // For contract: version_byte = 2 (shifted: 2 << 3 = 16)
    let version_byte: u8 = 2 << 3; // Contract = 'C'
    let mut payload = vec![version_byte];
    payload.extend_from_slice(hash.as_ref());

    // CRC16-XModem checksum
    let checksum = crc16_xmodem(&payload);
    payload.push((checksum & 0xFF) as u8);
    payload.push((checksum >> 8) as u8);

    // Base32 encode (no padding)
    base32_encode(&payload)
}

fn crc16_xmodem(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

fn base32_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut result = String::new();
    let mut buffer: u64 = 0;
    let mut bits = 0;

    for &byte in data {
        buffer = (buffer << 8) | byte as u64;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            result.push(ALPHABET[((buffer >> bits) & 0x1F) as usize] as char);
        }
    }
    if bits > 0 {
        buffer <<= 5 - bits;
        result.push(ALPHABET[(buffer & 0x1F) as usize] as char);
    }

    result
}

/// Extract events from a single transaction's `TransactionMeta`.
fn extract_events_from_tx_meta(
    tx_meta: &TransactionMeta,
    seq: u32,
    close_time: i64,
    tx_idx: u32,
    tx_hash: &str,
    events: &mut Vec<ExtractedEvent>,
) {
    match tx_meta {
        TransactionMeta::V3(v3) => {
            extract_events_from_v3(v3, seq, close_time, tx_idx, tx_hash, events);
        }
        TransactionMeta::V4(v4) => {
            extract_events_from_v4(v4, seq, close_time, tx_idx, tx_hash, events);
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
) {
    let (contract_id, event_type, topics, data) = extract_contract_event(contract_event);

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

/// Extract all events from a LedgerCloseMetaBatch.
pub fn extract_events(batch: &LedgerCloseMetaBatch) -> Vec<ExtractedEvent> {
    let mut events = Vec::new();

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
    fn test_event_id_roundtrip() {
        let id = event_id(58000000, EventPhase::Operation, 3, 7);
        assert_eq!(id, "evt_0058000000_1_0003_0_0007");
        let (seq, phase, tx, sub, evt) = parse_event_id(&id).unwrap();
        assert_eq!(seq, 58000000);
        assert_eq!(phase, 1);
        assert_eq!(tx, 3);
        assert_eq!(sub, 0);
        assert_eq!(evt, 7);
    }

    #[test]
    fn test_event_id_ordering() {
        let before = event_id(100, EventPhase::BeforeAllTxs, 0, 0);
        let op = event_id(100, EventPhase::Operation, 0, 0);
        let after_tx = event_id(100, EventPhase::AfterTx, 0, 0);
        let after_all = event_id(100, EventPhase::AfterAllTxs, 0, 0);
        assert!(before < op);
        assert!(op < after_tx);
        assert!(after_tx < after_all);
    }

    #[test]
    fn test_parse_invalid_event_id() {
        assert!(parse_event_id("invalid").is_none());
        assert!(parse_event_id("evt_abc_def_ghi_jkl").is_none());
        assert!(parse_event_id("evt_1_2").is_none());
        assert!(parse_event_id("evt_1_2_3").is_none());
    }

    #[test]
    fn test_event_type_display() {
        assert_eq!(EventType::Contract.to_string(), "contract");
        assert_eq!(EventType::System.to_string(), "system");
        assert_eq!(EventType::Diagnostic.to_string(), "diagnostic");
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let external = encode_event_id(58000000, 1, 3, 0, 7);
        assert!(external.starts_with("evt_"));
        // Verify it's URL-safe: only alphanumeric, '-', '_'
        let payload = external.strip_prefix("evt_").unwrap();
        assert!(payload
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));

        let (seq, phase, tx, sub, evt) = decode_event_id(&external).unwrap();
        assert_eq!(seq, 58000000);
        assert_eq!(phase, 1);
        assert_eq!(tx, 3);
        assert_eq!(sub, 0);
        assert_eq!(evt, 7);
    }

    #[test]
    fn test_encode_decode_all_phases() {
        for (phase, sub) in [(0, 0), (1, 0), (1, 1), (2, 0)] {
            let external = encode_event_id(100, phase, 5, sub, 10);
            let (s, p, t, u, e) = decode_event_id(&external).unwrap();
            assert_eq!((s, p, t, u, e), (100, phase, 5, sub, 10));
        }
    }

    #[test]
    fn test_decode_invalid_external_ids() {
        assert!(decode_event_id("invalid").is_none());
        assert!(decode_event_id("evt_").is_none());
        assert!(decode_event_id("evt_!!!").is_none());
        assert!(decode_event_id("evt_AAAA").is_none()); // too short after decode
    }

    #[test]
    fn test_to_external_and_back() {
        let internal = event_id(58000000, EventPhase::Operation, 3, 7);
        let external = to_external_id(&internal).unwrap();
        let back = to_internal_id(&external).unwrap();
        assert_eq!(back, internal);
    }

    #[test]
    fn test_to_internal_id_invalid() {
        assert!(to_internal_id("evt_bad").is_none());
        assert!(to_internal_id("not_an_id").is_none());
    }

    #[test]
    fn test_crc16_xmodem() {
        // Known test vector
        let data = b"123456789";
        assert_eq!(crc16_xmodem(data), 0x31C3);
    }
}
