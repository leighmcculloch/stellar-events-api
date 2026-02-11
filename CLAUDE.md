# CLAUDE.md

Guidelines for AI agents contributing to this repository.

## Build & check commands

Run after every change:

```bash
cargo fmt        # Format code
cargo clippy     # Lint — must produce zero warnings
cargo test       # Run all tests
```

`cargo clippy` must have zero warnings. Do not add `#[allow(...)]` unless
there is no reasonable way to restructure the code to fix the warning.

## Project structure

```
src/
  main.rs              # Entry point, CLI args, server startup
  lib.rs               # AppState, Error enum, public module declarations
  sync.rs              # Background ledger sync loop
  db.rs                # In-memory EventStore (DashMap), query logic, EventFilter, EventQueryParams
  api/
    mod.rs             # Router setup (axum)
    routes.rs          # Request handlers: list_events_get, list_events_post, get_event, health
    error.rs           # ApiError type and HTTP error responses
    types.rs           # Response types: Event, ListResponse, StatusResponse
    home.html          # Embedded HTML docs page (served at /)
  ledger/
    mod.rs             # Module declarations
    events.rs          # XDR event extraction: LedgerCloseMetaBatch → ExtractedEvent
    event_id.rs        # Event ID encoding/decoding (internal ↔ external opaque IDs)
    fetch.rs           # HTTP fetch + XDR parsing of ledger meta from S3
    path.rs            # S3 path construction, StoreConfig
tests/
  api_tests.rs         # Integration tests: spin up a real HTTP server, send requests
  cold_fetch_test.rs   # Latency benchmarks for cold-fetch pipeline
```

## Key architecture details

- **In-memory store**: `EventStore` in `db.rs` uses `DashMap` partitioned by
  ledger sequence. Reads are lock-free. There is no external database.
- **Event matching**: `StoredEvent::matches_filter` checks each `EventFilter`
  field (contract, type, topics). Filters within a `Vec<EventFilter>` are
  OR'd; fields within a single filter are AND'd.
- **Pagination**: Results are always descending (newest first). `before`
  paginates backward through older events. `after` returns events newer than
  the cursor.
- **Backfill**: When a query targets a ledger not yet cached, `backfill_if_needed`
  fetches it from S3 on demand before running the query.
- **HTML docs**: `home.html` is embedded at compile time via `include_str!`.
  Changes require recompilation.
- **Event IDs**: Internal IDs encode (ledger, phase, tx_index, event_index).
  External IDs are base58-encoded with an `evt_` prefix.

## Testing patterns

- **Integration tests** (`tests/api_tests.rs`): Use `start_test_server()` to
  bind a real TCP listener on a random port. Use `make_test_events()` and
  `make_multi_type_events()` to create test data. All events in a test must
  share a ledger sequence so they can be queried together.
- **Unit tests**: Placed in `#[cfg(test)] mod tests` inside each module.
- Tests must not depend on network access or external services.

## Common pitfalls

- When adding a new field to `EventFilter`, also update `matches_filter` in
  `db.rs` and add it to both `ListEventsRequest` and `list_events_get`
  parameter parsing in `routes.rs`.
- `EventFilter` derives `Default` and `serde::Deserialize` — new `Option`
  fields work automatically with `#[serde(default)]`.
- Topic values are XDR-JSON `ScVal` objects (e.g. `{"symbol":"transfer"}`),
  not plain strings. Tests must use this format.
- The `home.html` page has interactive "Try it" panels with JavaScript that
  builds curl commands and submits requests. When changing API parameters,
  update both the documentation tables and the example textarea values.
