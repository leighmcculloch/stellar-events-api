# stellar-events-api

An HTTP API for querying contract events from the Stellar network.

Reads ledger data from Stellar's public ledger metadata archive (per [SEP-54](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0054.md)), extracts contract events, and exposes them through a paginated REST API.

## Quick start

```bash
cargo run
```

The server starts on port 3000 by default and begins syncing ledgers from the Stellar pubnet. Events become queryable as ledgers are ingested.

## API

### List events

```
GET  /events
POST /events
```

Returns a paginated list of contract events. Parameters can be passed as query string parameters (GET) or as a JSON request body (POST).

**Parameters:**

| Parameter | Type | Description |
|---|---|---|
| `limit` | integer | Number of events to return (1-100, default 10) |
| `after` | string | Return events newer than this cursor (event ID) |
| `before` | string | Return events older than this cursor (event ID) |
| `ledger` | integer | Return events from this ledger sequence |
| `tx` | string | Limit results to events from this transaction hash |
| `q` | string | Filter query string (see syntax below). Cannot be combined with `filters`. |
| `filters` | array | Structured filters (legacy). Cannot be combined with `q`. |

**Query syntax (`q` parameter):** Filter events using `key:value` qualifiers. Space-separated qualifiers are AND'd. Use `OR` for alternatives. Parentheses group expressions. AND binds tighter than OR.

| Key | Value | Example |
|---|---|---|
| `type` | `contract`, `system`, or `diagnostic` | `type:contract` |
| `contract` | Stellar contract strkey (C...) | `contract:CCW67...` |
| `topic` | XDR-JSON ScVal object | `topic:{"symbol":"transfer"}` |
| `topic0`..`topic3` | XDR-JSON ScVal object | `topic0:{"symbol":"transfer"}` |

Example: find transfer events on either of two contracts:

```
GET /events?q=(contract:CABC... OR contract:CXYZ...) type:contract topic0:{"symbol":"transfer"}
```

**Query limits:**

| Limit | Value |
|---|---|
| Max query length | 1,024 bytes |
| Max terms (key:value pairs) | 20 |
| Max parenthesis nesting depth | 4 |
| Max filter combinations after expansion | 20 |

Results are always returned in descending order (newest first). If no `ledger` or cursor is provided, the API defaults to the latest ingested ledger.

**Examples:**

All events from the latest ledger onward:

```bash
curl 'http://localhost:3000/events'
```

All events from ledger 58000000 onward:

```bash
curl 'http://localhost:3000/events?ledger=58000000'
```

All events for the USDC contract:

```bash
curl 'http://localhost:3000/events?ledger=58000000&q=contract:CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75'
```

Transfer events for the USDC contract:

```bash
curl 'http://localhost:3000/events?ledger=58000000&q=contract:CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75%20topic0:%7B%22symbol%22:%22transfer%22%7D'
```

Same query using POST with a JSON body:

```bash
curl -X POST 'http://localhost:3000/events' \
  -H 'Content-Type: application/json' \
  -d '{
    "ledger": 58000000,
    "q": "contract:CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75 topic0:{\"symbol\":\"transfer\"}"
  }'
```

**Response:**

```json
{
  "object": "list",
  "url": "/events",
  "data": [
    {
      "object": "event",
      "id": "evt_0058000000_1_0000_0_0000",
      "ledger": 58000000,
      "at": "2024-01-15T12:00:00+00:00",
      "tx": "abc123...",
      "type": "contract",
      "contract": "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75",
      "topics": [{"symbol": "transfer"}],
      "data": {"i128": {"hi": 0, "lo": 1000000}}
    }
  ]
}
```

**Pagination:** Use the `next` URL or the `id` of the last item in `data` as the `after` value for the next page.

**Streaming new events:** Keep polling with the last seen `id` as `after`. New events will appear as the server syncs new ledgers.

**Error responses** use a structured format:

```json
{
  "error": {
    "type": "invalid_request_error",
    "code": "invalid_parameter",
    "message": "limit must be between 1 and 100",
    "param": "limit"
  }
}
```

### Server health

```
GET /health
```

Returns the server's sync state, including the latest ingested ledger.

### Prometheus metrics

```
GET /metrics
```

Returns metrics in Prometheus exposition format. Key metrics:

- `api_requests_total` — total API requests (by endpoint)
- `api_request_duration_seconds` — request latency histogram (by endpoint)
- `api_events_returned` — histogram of event counts per response
- `sync_ledgers_total` — total ledgers synced
- `sync_events_total` — total events ingested via sync
- `sync_latest_ledger` — latest synced ledger sequence
- `sync_errors_total` — total sync fetch errors
- `store_partitions_total` — current number of cached ledger partitions
- `store_events_ingested_total` — total events inserted into the store
- `store_partitions_expired_total` — total partitions removed by cache expiry

## Configuration

All configuration is via CLI flags or environment variables:

| Flag | Env | Default | Description |
|---|---|---|---|
| `--port` | `PORT` | `3000` | HTTP server port |
| `--bind` | `BIND_ADDRESS` | `0.0.0.0` | Bind address |
| `--meta-url` | `META_URL` | *(pubnet S3)* | Base URL for ledger metadata |
| `--start-ledger` | `START_LEDGER` | *(auto)* | Ledger sequence to start syncing from |
| `--parallel-fetches` | `PARALLEL_FETCHES` | `10` | Number of ledgers to fetch concurrently |
| `--cache-ttl-days` | `CACHE_TTL_DAYS` | `1` | How long to keep cached ledger data |

Log level is controlled via the `RUST_LOG` environment variable (e.g., `RUST_LOG=debug`).

## Docker

```bash
docker build -t stellar-events-api .
docker run -p 3000:3000 stellar-events-api
```

## Design

- **Data source**: Reads compressed XDR ledger metadata from the Stellar public S3 archive per the SEP-54 specification. No AWS SDK or S3 libraries are used; all access is via plain HTTP.
- **Caching**: Each ledger's data is cached in-memory for the configured TTL (default 1 day). Expired partitions are dropped instantly.
- **Proactive sync**: A background task continuously polls for new ledgers and indexes their events as they appear on the archive. On startup, it discovers the current network ledger from Horizon.
- **Storage**: Events are stored in-memory, partitioned by ledger sequence. Each partition is an immutable snapshot behind an `Arc`, enabling lock-free concurrent reads with zero serialisation overhead.
- **XDR representation**: Contract event XDR is serialized using the xdr-json format provided by the `stellar-xdr` crate, matching the Stellar ecosystem's standard JSON representation.
- **API style**: The REST API uses cursor-based pagination, consistent list envelopes, and structured error responses.

## Development

```bash
# Run tests
cargo test

# Run with debug logging
RUST_LOG=debug cargo run

# Check formatting and lints
cargo fmt --check
cargo clippy
```

## License

Copyright 2026 Stellar Development Foundation (This is not an official project of the Stellar Development Foundation)

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
