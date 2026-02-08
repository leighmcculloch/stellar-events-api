# Optimization Experiments

**Goal:** 50% improvement in cold-fetch latency (baseline p50: ~0.65ms)
**Test:** `cargo test --release --test cold_fetch_test -- --nocapture`

| # | Experiment | p50 (ms) | Change vs baseline | Kept? |
|---|-----------|----------|-------------------|-------|
| - | Baseline  | 0.65     | -                 | -     |
| 1 | Store serde_json::Value directly instead of JSON strings (eliminate Value→String→Value roundtrip) | 0.93 | Pipeline: 151µs→140µs (-7%), JSON serialize: 38µs→21µs (-45%) | Yes |
