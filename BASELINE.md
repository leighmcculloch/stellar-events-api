# Cold Fetch Latency Baseline

**Test:** `tests/cold_fetch_test.rs` — `test_cold_fetch_latency`
**Workload:** 10 transactions x 5 events = 50 events per ledger, compressed XDR (244 bytes)
**Path exercised:** HTTP request → backfill → fetch from mock S3 → zstd decompress → XDR parse → event extraction → store insert → query → JSON response
**Mode:** `cargo test --release`
**Iterations per run:** 20 (fresh server each iteration)

## Results (5 runs)

| Run | min (ms) | p50 (ms) | avg (ms) | p95 (ms) | max (ms) |
|-----|----------|----------|----------|----------|----------|
| 1   | 0.47     | 0.78     | 2.55     | 35.14    | 35.14    |
| 2   | 0.45     | 0.58     | 2.29     | 32.88    | 32.88    |
| 3   | 0.45     | 0.74     | 2.34     | 32.67    | 32.67    |
| 4   | 0.42     | 0.66     | 2.17     | 28.55    | 28.55    |
| 5   | 0.44     | 0.56     | 2.39     | 35.88    | 35.88    |

## Summary

| Metric | Value |
|--------|-------|
| **Steady-state p50** | ~0.65ms |
| **Steady-state min** | ~0.44ms |
| **First-request cold start** | ~30-35ms |

The p95/max is dominated by the first iteration's TCP connection establishment to the mock S3 server (reqwest Client connection pool is cold). Steady-state performance after the first request is 0.4-0.8ms.

The avg (~2.3ms) is skewed by the single cold-start outlier per run.
