# Optimization Experiments

**Goal:** 50% improvement in cold-fetch latency (baseline p50: ~0.65ms)
**Test:** `cargo test --release --test cold_fetch_test -- --nocapture`

| # | Experiment | p50 (ms) | Change vs baseline | Kept? |
|---|-----------|----------|-------------------|-------|
| - | Baseline  | 0.65     | -                 | -     |
| 1 | Store serde_json::Value directly instead of JSON strings (eliminate Value→String→Value roundtrip) | 0.93 | Pipeline: 151µs→140µs (-7%), JSON serialize: 38µs→21µs (-45%) | Yes |
| 2 | Replace hand-rolled base32+CRC16 strkey with stellar-strkey crate | 0.74 | Pipeline: 140µs→122µs (-13%), event extraction: 42µs→23µs (-45%) | Yes |
| 3 | Bulk zstd decompress instead of streaming decoder | 0.78 | Pipeline: 122µs→108µs (-11%), fetch+decompress: 36µs→21µs (-42%) | Yes |
| 4 | Pre-compute external IDs, timestamps, event_type during insertion | 0.69 | JSON serialize: 22µs→12µs (-45%), store insert: 20µs→30µs (+50%), net TOTAL: 108µs→107µs | Yes |
| 5 | Use Arc\<str\> for tx_hash to share across events in same tx | 0.69 | No meaningful change (noise), adds complexity | No |
| 6 | Move-semantics for insert_events (avoid cloning topics/data Values) | 0.62 | Pipeline: 107µs→95µs (-11%), store insert: 30µs→20µs (-33%) | Yes |
| 7 | Pre-allocate Vec in extract_events with tx_count heuristic | 0.62 | No measurable change at 50 events, good practice for larger workloads | Yes |
| 8 | Cache contract ID strkey encoding across events in same batch | 0.62 | event extraction: 23µs→20µs (-13%), bigger gains for real ledgers with repeated contracts | Yes |
