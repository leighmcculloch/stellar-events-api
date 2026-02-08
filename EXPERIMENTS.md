# Optimization Experiments

**Goal:** 50% improvement in cold-fetch latency (baseline p50: ~0.65ms)
**Test:** `cargo test --release --test cold_fetch_test -- --nocapture`

| # | Experiment | p50 (ms) | Change vs baseline | Kept? |
|---|-----------|----------|-------------------|-------|
| - | Baseline  | 0.65     | -                 | -     |
| 1 | Store serde_json::Value directly instead of JSON strings (eliminate Value→String→Value roundtrip) | 0.93 | Pipeline: 151µs→140µs (-7%), JSON serialize: 38µs→21µs (-45%) | Yes |
| 2 | Replace hand-rolled base32+CRC16 strkey with stellar-strkey crate | 0.74 | Pipeline: 140µs→122µs (-13%), event extraction: 42µs→23µs (-45%) | Yes |
| 3 | Bulk zstd decompress instead of streaming decoder | 0.78 | Pipeline: 122µs→108µs (-11%), fetch+decompress: 36µs→21µs (-42%) | Yes |
