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

/// Two large odd multipliers for obfuscating event IDs. A bit-reversal between
/// the two multiplications gives full bidirectional diffusion: the first multiply
/// propagates low bits upward, the reversal swaps high↔low, and the second
/// multiply propagates again — so every input bit affects every output bit.
const MULTIPLIER_A: u128 = 0xb5a4_f317_8d2e_c906_4bf1_73a8_e5d1;
const MULTIPLIER_B: u128 = 0xe8f2_7c94_a1d5_630b_9e4a_8d17_b3f9;

/// Mask for 112-bit (14-byte) values.
const MOD_MASK: u128 = (1u128 << 112) - 1;

/// Modular multiplicative inverses for decoding.
const INVERSE_A: u128 = mod_inverse(MULTIPLIER_A);
const INVERSE_B: u128 = mod_inverse(MULTIPLIER_B);

/// Compute the modular multiplicative inverse of an odd number mod 2^112
/// using Newton's method. Each iteration doubles the number of correct bits,
/// so 7 iterations (1 → 2 → 4 → 8 → 16 → 32 → 64 → 128 ≥ 112) suffice.
const fn mod_inverse(m: u128) -> u128 {
    let mask = (1u128 << 112) - 1;
    let mut x: u128 = 1;
    let mut i = 0;
    while i < 7 {
        x = x.wrapping_mul(2u128.wrapping_sub(m.wrapping_mul(x))) & mask;
        i += 1;
    }
    x
}

/// Custom base32 alphabet: 26 lowercase + 6 visually distinct uppercase.
const BASE32_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyzBDGNRT";

/// Encode 14 bytes as 23 base32 characters using the custom alphabet.
fn base32_encode(data: &[u8; 14]) -> String {
    let mut result = String::with_capacity(23);
    let mut buf: u64 = 0;
    let mut bits = 0u32;

    for &byte in data.iter() {
        buf = (buf << 8) | byte as u64;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            result.push(BASE32_ALPHABET[((buf >> bits) & 0x1f) as usize] as char);
        }
    }

    if bits > 0 {
        result.push(BASE32_ALPHABET[((buf << (5 - bits)) & 0x1f) as usize] as char);
    }

    result
}

/// Decode 23 base32 characters back into 14 bytes.
fn base32_decode(s: &str) -> Option<[u8; 14]> {
    if s.len() != 23 {
        return None;
    }
    let mut result = [0u8; 14];
    let mut buf: u64 = 0;
    let mut bits = 0u32;
    let mut i = 0;

    for ch in s.bytes() {
        let val = BASE32_ALPHABET.iter().position(|&c| c == ch)? as u64;
        buf = (buf << 5) | val;
        bits += 5;
        while bits >= 8 && i < 14 {
            bits -= 8;
            result[i] = ((buf >> bits) & 0xff) as u8;
            i += 1;
        }
    }

    if i != 14 {
        return None;
    }

    Some(result)
}

/// Encode event ID components into an opaque external format.
///
/// Packs 5 components into 14 bytes big-endian, treats them as a 112-bit
/// integer, and obfuscates via multiply → bit-reverse → multiply for full
/// diffusion. Then base32 encodes with a custom letter-only alphabet and
/// `evt_` prefix.
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

    let mut padded = [0u8; 16];
    padded[2..16].copy_from_slice(&buf);
    let mut val = u128::from_be_bytes(padded);
    val = val.wrapping_mul(MULTIPLIER_A) & MOD_MASK;
    val = val.reverse_bits() >> 16;
    val = val.wrapping_mul(MULTIPLIER_B) & MOD_MASK;
    buf.copy_from_slice(&val.to_be_bytes()[2..16]);

    let encoded = base32_encode(&buf);
    format!("evt_{}", encoded)
}

/// Decode an opaque external event ID back into its components.
pub fn decode_event_id(id: &str) -> Option<(u32, u8, u32, u8, u32)> {
    let payload = id.strip_prefix("evt_")?;
    let buf = base32_decode(payload)?;

    let mut padded = [0u8; 16];
    padded[2..16].copy_from_slice(&buf);
    let mut val = u128::from_be_bytes(padded);
    val = val.wrapping_mul(INVERSE_B) & MOD_MASK;
    val = val.reverse_bits() >> 16;
    val = val.wrapping_mul(INVERSE_A) & MOD_MASK;
    let bytes = val.to_be_bytes();

    let ledger_sequence = u32::from_be_bytes(bytes[2..6].try_into().ok()?);
    let phase = bytes[6];
    let tx_index = u32::from_be_bytes(bytes[7..11].try_into().ok()?);
    let sub = bytes[11];
    let event_index = u32::from_be_bytes(bytes[12..16].try_into().ok()?);

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
    fn test_encode_decode_roundtrip() {
        let external = encode_event_id(58000000, 1, 3, 0, 7);
        assert!(external.starts_with("evt_"));
        // Verify payload is letters only (custom base32 alphabet).
        let payload = external.strip_prefix("evt_").unwrap();
        assert_eq!(payload.len(), 23);
        assert!(payload.chars().all(|c| c.is_ascii_alphabetic()));

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
}
