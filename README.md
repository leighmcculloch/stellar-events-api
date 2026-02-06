# stellar-events-api

An HTTP API for querying contract events from the Stellar network.

Reads ledger data from Stellar's public ledger metadata archive (per [SEP-54](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0054.md)), extracts contract events, and exposes them through a paginated REST API modeled after [Stripe's API conventions](https://docs.stripe.com/api).

## Quick start

```bash
cargo run
```

The server starts on port 3000 by default and begins syncing ledgers from the Stellar pubnet. Events become queryable as ledgers are ingested.

## API

### List events

```
GET  /v1/events
POST /v1/events
```

Returns a paginated list of contract events. Parameters can be passed as query string parameters (GET) or as a JSON request body (POST).

**Parameters:**

| Parameter | Type | Description |
|---|---|---|
| `limit` | integer | Number of events to return (1-100, default 10) |
| `start_after` | string | Cursor for forward pagination (event ID) |
| `start_ledger` | integer | Return events from this ledger sequence onward (inclusive) |
| `filters` | array | Structured filters (see below). JSON-encoded string for GET, native array for POST |

**Filters:** The `filters` parameter accepts a JSON-encoded array of filter objects. Each filter in the array is OR'd together; conditions within a single filter are AND'd. This enables complex queries like "transfer events on contract A, OR mint events on contract B".

Each filter object supports:

| Field | Type | Description |
|---|---|---|
| `contract_id` | string | Match events from this contract ID |
| `type` | string | Match events of this type (`contract`, `system`, `diagnostic`) |
| `tx_hash` | string | Match events from this transaction hash |
| `topics` | array | Positional topic matching. Each element is an XDR-JSON ScVal or `"*"` (wildcard) |

Topics are matched by position: element 0 of the filter matches against topic 0 of the event, element 1 against topic 1, and so on. The string `"*"` matches any value at that position. The event must have at least as many topics as the filter specifies. Topic values are XDR-JSON ScVals as serialized by the `stellar-xdr` crate (e.g. `{"symbol":"transfer"}`, `{"address":"GABC..."}`).

Example: find transfer events to a specific address on either of two contracts:

```
GET /v1/events?filters=[
  {"contract_id":"CABC...","type":"contract","topics":[{"symbol":"transfer"},"*",{"address":"GDEF..."}]},
  {"contract_id":"CXYZ...","type":"contract","topics":[{"symbol":"transfer"},"*",{"address":"GDEF..."}]}
]
```

If no `start_ledger` or `start_after` is provided, the API defaults to the latest ingested ledger.

**Examples:**

All events from the latest ledger onward:

```bash
curl 'http://localhost:3000/v1/events'
```

All events from ledger 58000000 onward:

```bash
curl 'http://localhost:3000/v1/events?start_ledger=58000000'
```

All events for the USDC contract:

```bash
curl 'http://localhost:3000/v1/events?start_ledger=58000000&filters=[{"contract_id":"CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75"}]'
```

Transfer events for the USDC contract:

```bash
curl 'http://localhost:3000/v1/events?start_ledger=58000000&filters=[{"contract_id":"CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75","topics":[{"symbol":"transfer"}]}]'
```

Same query using POST with a JSON body:

```bash
curl -X POST 'http://localhost:3000/v1/events' \
  -H 'Content-Type: application/json' \
  -d '{
    "start_ledger": 58000000,
    "filters": [
      {
        "contract_id": "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75",
        "topics": [{"symbol": "transfer"}]
      }
    ]
  }'
```

**Response:**

```json
{
  "object": "list",
  "url": "/v1/events",
  "has_more": true,
  "data": [
    {
      "object": "event",
      "id": "evt_0058000000_1_0000_0_0000",
      "ledger_sequence": 58000000,
      "ledger_closed_at": "2024-01-15T12:00:00+00:00",
      "tx_hash": "abc123...",
      "type": "contract",
      "contract_id": "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75",
      "topics": [{"symbol": "transfer"}],
      "data": {"i128": {"hi": 0, "lo": 1000000}}
    }
  ]
}
```

**Pagination:** Use the `id` of the last item in `data` as the `start_after` value for the next page. Iterate until `has_more` is `false`.

**Streaming new events:** Paginate forward until `has_more` is `false`, then keep polling with the last seen `id` as `start_after`. New events will appear as the server syncs new ledgers.

**Error responses** follow Stripe's format:

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

### Server status

```
GET /v1/status
```

Returns the server's sync state, including the latest ingested ledger.

## Configuration

All configuration is via CLI flags or environment variables:

| Flag | Env | Default | Description |
|---|---|---|---|
| `--port` | `PORT` | `3000` | HTTP server port |
| `--bind` | `BIND_ADDRESS` | `0.0.0.0` | Bind address |
| `--data-dir` | `DATA_DIR` | `./data` | Directory for the SQLite database |
| `--meta-url` | `META_URL` | *(pubnet S3)* | Base URL for ledger metadata |
| `--start-ledger` | `START_LEDGER` | *(auto)* | Ledger sequence to start syncing from |

Log level is controlled via the `RUST_LOG` environment variable (e.g., `RUST_LOG=debug`).

## Docker

```bash
docker build -t stellar-events-api .
docker run -p 3000:3000 -v stellar-data:/data stellar-events-api
```

## Design

- **Data source**: Reads compressed XDR ledger metadata from the Stellar public S3 archive per the SEP-54 specification. No AWS SDK or S3 libraries are used; all access is via plain HTTP.
- **Caching**: Each ledger's data is cached for 7 days after first retrieval. Expired entries are automatically cleaned up.
- **Proactive sync**: A background task continuously polls for new ledgers and indexes their events as they appear on the archive. On startup, it resumes from the last synced position or discovers the current network ledger from Horizon.
- **Storage**: Events are stored in a local SQLite database with indexes on ledger sequence, contract ID, and event type.
- **XDR representation**: Contract event XDR is serialized using the xdr-json format provided by the `stellar-xdr` crate, matching the Stellar ecosystem's standard JSON representation.
- **API style**: The REST API follows Stripe's conventions: cursor-based pagination, consistent list envelopes, structured error responses.

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

Apache-2.0
