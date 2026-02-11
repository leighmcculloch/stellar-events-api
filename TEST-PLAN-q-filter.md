# Test Plan: `q=` Filter Query Syntax

This document specifies every test case needed to verify the `q=` filter query parameter described in `DESIGN-q-filter.md`. Each test is self-contained: another engineer should be able to implement it from this plan alone.

---

## Test Data Reference

All integration tests reuse the existing `make_multi_type_events()` fixture (5 events on ledger 100):

| Index | Type       | Contract    | Topics                                                              |
|-------|------------|-------------|---------------------------------------------------------------------|
| 0     | contract   | CA...AA (56 chars) | `[{"symbol":"transfer"}, {"address":"GABC"}, {"address":"GDEF"}]` |
| 1     | system     | None        | `[{"symbol":"core_metrics"}]`                                       |
| 2     | contract   | CB...BB (56 chars) | `[{"symbol":"transfer"}, {"address":"GCCC"}, {"address":"GDDD"}]` |
| 3     | contract   | CA...AA (56 chars) | `[{"symbol":"mint"}, {"address":"GABC"}]`                          |
| 4     | diagnostic | CA...AA (56 chars) | `[{"symbol":"diag"}]`                                              |

Full contract IDs:
- **CA**: `CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA`
- **CB**: `CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB`

---

## Part 1: Parser Unit Tests (`src/api/query_parser.rs`)

These test `parse_query(input) -> Result<Vec<EventFilter>, QueryParseError>` directly. They do not start a server.

### 1.1 Single qualifier -- type

**Test name:** `test_parse_single_type_contract`

- **Input:** `"type:contract"`
- **Expected:** `Ok(vec)` with one `EventFilter`:
  - `event_type: Some("contract")`
  - `contract_id: None`
  - `topics: None`

**Test name:** `test_parse_single_type_system`

- **Input:** `"type:system"`
- **Expected:** `Ok(vec)` with one `EventFilter`:
  - `event_type: Some("system")`

**Test name:** `test_parse_single_type_diagnostic`

- **Input:** `"type:diagnostic"`
- **Expected:** `Ok(vec)` with one `EventFilter`:
  - `event_type: Some("diagnostic")`

### 1.2 Single qualifier -- contract

**Test name:** `test_parse_single_contract`

- **Input:** `"contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"`
- **Expected:** `Ok(vec)` with one `EventFilter`:
  - `contract_id: Some("CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")`
  - `event_type: None`
  - `topics: None`

### 1.3 Single qualifier -- topic0

**Test name:** `test_parse_single_topic0`

- **Input:** `r#"topic0:{"symbol":"transfer"}"#`
- **Expected:** `Ok(vec)` with one `EventFilter`:
  - `topics: Some(vec![json!({"symbol":"transfer"})])`
  - `event_type: None`
  - `contract_id: None`

### 1.4 Topic with nested JSON

**Test name:** `test_parse_topic_nested_json`

- **Input:** `r#"topic0:{"nested":{"a":"b"}}"#`
- **Expected:** `Ok(vec)` with one `EventFilter`:
  - `topics: Some(vec![json!({"nested":{"a":"b"}})])`

### 1.5 AND (implicit) -- type + contract

**Test name:** `test_parse_and_type_contract`

- **Input:** `"type:contract contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"`
- **Expected:** `Ok(vec)` with one `EventFilter`:
  - `event_type: Some("contract")`
  - `contract_id: Some("CA...")`
  - `topics: None`

### 1.6 AND -- type + topic0 + topic2 (gap at position 1)

**Test name:** `test_parse_and_type_topic0_topic2`

- **Input:** `r#"type:contract topic0:{"symbol":"transfer"} topic2:{"address":"GDEF"}"#`
- **Expected:** `Ok(vec)` with one `EventFilter`:
  - `event_type: Some("contract")`
  - `topics: Some(vec![json!({"symbol":"transfer"}), Value::Null, json!({"address":"GDEF"})])`

### 1.7 OR -- two types

**Test name:** `test_parse_or_two_types`

- **Input:** `"type:contract OR type:system"`
- **Expected:** `Ok(vec)` with two `EventFilter`s:
  - `[0]`: `event_type: Some("contract")`
  - `[1]`: `event_type: Some("system")`

### 1.8 OR -- three-way

**Test name:** `test_parse_or_three_way`

- **Input:** `"type:contract OR type:system OR type:diagnostic"`
- **Expected:** `Ok(vec)` with three `EventFilter`s, one per type.

### 1.9 Parenthesized group -- OR inside AND

**Test name:** `test_parse_paren_or_and`

- **Input:** `r#"(type:contract OR type:system) contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"#`
- **Expected:** `Ok(vec)` with two `EventFilter`s (DNF expansion):
  - `[0]`: `event_type: Some("contract")`, `contract_id: Some("CA...")`
  - `[1]`: `event_type: Some("system")`, `contract_id: Some("CA...")`

### 1.10 Parenthesized group -- OR contracts with shared topic

**Test name:** `test_parse_paren_or_contracts_topic`

- **Input:** `r#"(contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA OR contract:CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB) topic0:{"symbol":"transfer"}"#`
- **Expected:** `Ok(vec)` with two `EventFilter`s:
  - `[0]`: `contract_id: Some("CA...")`, `topics: Some(vec![json!({"symbol":"transfer"})])`
  - `[1]`: `contract_id: Some("CB...")`, `topics: Some(vec![json!({"symbol":"transfer"})])`

### 1.11 Nested parentheses

**Test name:** `test_parse_nested_parens`

- **Input:** `"((type:contract))"`
- **Expected:** Same as `"type:contract"` -- one `EventFilter` with `event_type: Some("contract")`.

### 1.12 DNF cartesian product

**Test name:** `test_parse_dnf_cartesian_product`

- **Input:** `"(type:contract OR type:system) (contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA OR contract:CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB)"`
- **Expected:** `Ok(vec)` with 4 `EventFilter`s:
  - `{type:contract, contract:CA}`
  - `{type:contract, contract:CB}`
  - `{type:system, contract:CA}`
  - `{type:system, contract:CB}`

### 1.13 Duplicate qualifier -- same value collapsed

**Test name:** `test_parse_duplicate_same_value`

- **Input:** `"type:contract type:contract"`
- **Expected:** `Ok(vec)` with one `EventFilter`:
  - `event_type: Some("contract")`

### 1.14 Precedence -- AND binds tighter than OR

**Test name:** `test_parse_precedence_and_over_or`

- **Input:** `r#"type:contract topic0:{"symbol":"transfer"} OR type:system topic0:{"symbol":"core_metrics"}"#`
- **Expected:** `Ok(vec)` with two `EventFilter`s:
  - `[0]`: `event_type: Some("contract")`, `topics: Some(vec![json!({"symbol":"transfer"})])`
  - `[1]`: `event_type: Some("system")`, `topics: Some(vec![json!({"symbol":"core_metrics"})])`

### 1.15 Extra whitespace

**Test name:** `test_parse_extra_whitespace`

- **Input:** `"  type:contract  "`
- **Expected:** `Ok(vec)` with one `EventFilter`:
  - `event_type: Some("contract")`

### 1.16 Quoted value

**Test name:** `test_parse_quoted_value`

- **Input:** `r#"type:"contract""#`
- **Expected:** `Ok(vec)` with one `EventFilter`:
  - `event_type: Some("contract")`
  - Quotes are stripped before interpretation.

---

### 1.20 Error cases

Each test calls `parse_query(input)` and asserts `Err(e)` with the specified `e.kind`.

**Test name:** `test_parse_error_empty_query`

- **Input:** `""`
- **Expected error kind:** `empty_query`

**Test name:** `test_parse_error_whitespace_only`

- **Input:** `"   "`
- **Expected error kind:** `empty_query`

**Test name:** `test_parse_error_unknown_key`

- **Input:** `"foo:bar"`
- **Expected error kind:** `unknown_key`
- **Expected message contains:** `"foo"`

**Test name:** `test_parse_error_missing_value`

- **Input:** `"type:"`
- **Expected error kind:** `missing_value`

**Test name:** `test_parse_error_invalid_type_value`

- **Input:** `"type:bogus"`
- **Expected error kind:** `invalid_value`

**Test name:** `test_parse_error_invalid_type_wrong_case`

- **Input:** `"type:CONTRACT"`
- **Expected error kind:** `invalid_value`

**Test name:** `test_parse_error_unbalanced_open_paren`

- **Input:** `"(type:contract"`
- **Expected error kind:** `unbalanced_parens`

**Test name:** `test_parse_error_unbalanced_close_paren`

- **Input:** `"type:contract)"`
- **Expected error kind:** `unbalanced_parens`

**Test name:** `test_parse_error_empty_parens`

- **Input:** `"()"`
- **Expected error kind:** `unexpected_token`

**Test name:** `test_parse_error_leading_or`

- **Input:** `"OR type:contract"`
- **Expected error kind:** `unexpected_token`

**Test name:** `test_parse_error_trailing_or`

- **Input:** `"type:contract OR"`
- **Expected error kind:** `unexpected_token`

**Test name:** `test_parse_error_consecutive_or`

- **Input:** `"type:contract OR OR type:system"`
- **Expected error kind:** `unexpected_token`

**Test name:** `test_parse_error_conflicting_qualifiers`

- **Input:** `"type:contract type:system"`
- **Expected error kind:** `conflicting_qualifiers`

**Test name:** `test_parse_error_duplicate_topic_position`

- **Input:** `r#"topic0:{"symbol":"a"} topic0:{"symbol":"b"}"#`
- **Expected error kind:** `duplicate_topic_position`

**Test name:** `test_parse_error_unbalanced_braces`

- **Input:** `r#"topic0:{"symbol":"transfer""#`
- **Expected error kind:** `unbalanced_braces`

**Test name:** `test_parse_error_unbalanced_quotes`

- **Input:** `r#"type:"contract"#`
- **Expected error kind:** `unbalanced_quotes`

**Test name:** `test_parse_error_too_many_filters`

- **Input:** Build a query with enough OR/AND groups to produce >20 filters from DNF expansion, e.g.:
  ```
  (type:contract OR type:system OR type:diagnostic) (contract:CA... OR contract:CB...) (topic0:{"symbol":"transfer"} OR topic0:{"symbol":"mint"} OR topic0:{"symbol":"diag"} OR topic0:{"symbol":"core_metrics"})
  ```
  This produces 3 * 2 * 4 = 24 filters > 20.
- **Expected error kind:** `too_many_filters`

**Test name:** `test_parse_error_conflicting_qualifiers_in_paren_group`

- **Input:** `"(type:contract type:system)"`
- **Expected error kind:** `conflicting_qualifiers`
- Note: Even inside parens, the AND group contains conflicting type values.

---

## Part 2: API Integration Tests (`tests/api_tests.rs`)

These start a test server with `make_multi_type_events()` and exercise the HTTP API.

### 2.1 q with type filter (replaces `test_filter_by_type`)

**Test name:** `test_q_filter_by_type`

- **Setup:** `make_multi_type_events()`
- **Request:** `GET /events?ledger=100&q=type:system`
- **Expected:**
  - Status: 200
  - `data` length: 1
  - `data[0].type == "system"`

### 2.2 q with contract filter (replaces `test_filter_by_contract_id`)

**Test name:** `test_q_filter_by_contract`

- **Setup:** `make_multi_type_events()`
- **Request:** `GET /events?ledger=100&q=contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA`
- **Expected:**
  - Status: 200
  - `data` length: 3 (events 0, 3, 4 are on CA)
  - Every event has `contract == "CA..."`

### 2.3 q with topic filter (replaces `test_filter_by_topic_positional`)

**Test name:** `test_q_filter_by_topic`

- **Setup:** `make_multi_type_events()`
- **Request:** `GET /events?ledger=100&q=topic0:{"symbol":"transfer"}`
  - URL-encode the `q` value: `q=topic0%3A%7B%22symbol%22%3A%22transfer%22%7D`
- **Expected:**
  - Status: 200
  - `data` length: 2 (events 0 and 2)

### 2.4 q with topic wildcard positions (replaces `test_filter_topic_with_wildcard`)

**Test name:** `test_q_filter_topic_with_wildcard`

- **Setup:** `make_multi_type_events()`
- **Request:** `GET /events?ledger=100&q=topic0:{"symbol":"transfer"} topic2:{"address":"GDEF"}`
  - URL-encoded appropriately.
- **Expected:**
  - Status: 200
  - `data` length: 1 (event 0 only -- topic1 is wildcard, topic2 matches GDEF)
  - `data[0].topics[2].address == "GDEF"`

### 2.5 q with OR logic (replaces `test_filters_or_logic`)

**Test name:** `test_q_filter_or_logic`

- **Setup:** `make_multi_type_events()`
- **Request:** `GET /events?ledger=100&q=(contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA topic0:{"symbol":"transfer"}) OR (contract:CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB topic0:{"symbol":"transfer"})`
  - URL-encoded appropriately.
- **Expected:**
  - Status: 200
  - `data` length: 2 (events 0 and 2)

### 2.6 q with AND logic (replaces `test_filters_and_logic_within`)

**Test name:** `test_q_filter_and_logic`

- **Setup:** `make_multi_type_events()`
- **Request:** `GET /events?ledger=100&q=contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA type:contract topic0:{"symbol":"mint"}`
  - URL-encoded appropriately.
- **Expected:**
  - Status: 200
  - `data` length: 1 (event 3 only)

### 2.7 q combined with pagination (replaces `test_filters_combined_with_pagination`)

**Test name:** `test_q_filter_combined_with_pagination`

- **Setup:** `make_multi_type_events()`
- **Request 1:** `GET /events?ledger=100&limit=1&q=type:contract`
- **Expected 1:**
  - Status: 200
  - `data` length: 1
  - `next` cursor is present

- **Request 2:** `GET /events?limit=1&before={next}&q=type:contract`
  - Use the `next` cursor from request 1 as the `before` parameter.
- **Expected 2:**
  - Status: 200
  - `data` length: 1
  - Different event than request 1

### 2.8 q combined with ledger (replaces `test_filters_combined_with_ledger`)

**Test name:** `test_q_filter_combined_with_ledger`

- **Setup:** `make_multi_type_events()`
- **Request:** `GET /events?ledger=100&q=topic0:{"symbol":"transfer"}`
- **Expected:**
  - Status: 200
  - `data` length: 2 (events 0 and 2)

### 2.9 q with parenthesized group (new)

**Test name:** `test_q_filter_paren_group`

- **Setup:** `make_multi_type_events()`
- **Request:** `GET /events?ledger=100&q=(contract:CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA OR contract:CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB) topic0:{"symbol":"transfer"}`
  - URL-encoded.
- **Expected:**
  - Status: 200
  - `data` length: 2 (events 0 and 2 -- transfer on CA or CB)

### 2.10 q with invalid syntax returns 400 (new)

**Test name:** `test_q_filter_invalid_syntax`

- **Setup:** `start_test_server(vec![])`
- **Request:** `GET /events?q=foo:bar`
- **Expected:**
  - Status: 400
  - `error.type == "invalid_request_error"`
  - `error.code == "invalid_parameter"`
  - `error.param == "q"`
  - `error.message` contains `"unknown key"`

### 2.11 q via POST JSON body (new)

**Test name:** `test_q_filter_post_json`

- **Setup:** `make_multi_type_events()`
- **Request:** `POST /events` with JSON body:
  ```json
  {
    "ledger": 100,
    "q": "type:system"
  }
  ```
- **Expected:**
  - Status: 200
  - `data` length: 1
  - `data[0].type == "system"`

### 2.12 No q returns all events (new)

**Test name:** `test_no_q_returns_all`

- **Setup:** `make_multi_type_events()`
- **Request:** `GET /events?ledger=100`
- **Expected:**
  - Status: 200
  - `data` length: 5 (all events)

### 2.13 Both q and filters returns 400 (new)

**Test name:** `test_q_and_filters_conflict`

- **Setup:** `start_test_server(vec![])`
- **Request (GET):** `GET /events?q=type:contract&filters=[{"type":"system"}]`
  - URL-encode `filters` value.
- **Expected:**
  - Status: 400
  - `error.message` contains `"filters and q cannot both be provided"`
  - `error.param == "q"`

**Test name:** `test_q_and_filters_conflict_post`

- **Setup:** `start_test_server(vec![])`
- **Request (POST):** `POST /events` with JSON body:
  ```json
  {
    "q": "type:contract",
    "filters": [{"type":"system"}]
  }
  ```
- **Expected:**
  - Status: 400
  - `error.message` contains `"filters and q cannot both be provided"`

### 2.14 URL-encoded q with special characters (new)

**Test name:** `test_q_filter_url_encoded`

- **Setup:** `make_multi_type_events()`
- **Request:** `GET /events?ledger=100&q=type%3Acontract%20topic0%3A%7B%22symbol%22%3A%22transfer%22%7D`
  - This decodes to: `type:contract topic0:{"symbol":"transfer"}`
- **Expected:**
  - Status: 200
  - `data` length: 2 (events 0 and 2)

### 2.15 Empty q value returns error (new)

**Test name:** `test_q_filter_empty_value`

- **Setup:** `start_test_server(vec![])`
- **Request:** `GET /events?q=`
- **Expected:**
  - Status: 400
  - `error.param == "q"`
  - `error.message` contains `"empty"` or related text

### 2.16 q with diagnostic type (new)

**Test name:** `test_q_filter_diagnostic`

- **Setup:** `make_multi_type_events()`
- **Request:** `GET /events?ledger=100&q=type:diagnostic`
- **Expected:**
  - Status: 200
  - `data` length: 1 (event 4)
  - `data[0].type == "diagnostic"`

### 2.17 q with multiple topic positions including gap (new)

**Test name:** `test_q_filter_topic_gap`

- **Setup:** `make_multi_type_events()`
- **Request:** `GET /events?ledger=100&q=topic0:{"symbol":"transfer"} topic2:{"address":"GDDD"}`
  - URL-encoded.
- **Expected:**
  - Status: 200
  - `data` length: 1 (event 2 -- topic0=transfer, topic1=wildcard, topic2=GDDD)

### 2.18 q with three-way OR (new)

**Test name:** `test_q_filter_three_way_or`

- **Setup:** `make_multi_type_events()`
- **Request:** `GET /events?ledger=100&q=type:contract OR type:system OR type:diagnostic`
  - URL-encoded.
- **Expected:**
  - Status: 200
  - `data` length: 5 (all events match one of the three types)

### 2.19 q parse error returns structured error (new)

**Test name:** `test_q_filter_parse_error_format`

- **Setup:** `start_test_server(vec![])`
- **Request:** `GET /events?q=(type:contract`
- **Expected:**
  - Status: 400
  - Response body has: `error.type`, `error.code`, `error.message`, `error.param`
  - `error.type == "invalid_request_error"`
  - `error.param == "q"`

### 2.20 q filter no match returns empty data (new)

**Test name:** `test_q_filter_no_match`

- **Setup:** `make_multi_type_events()`
- **Request:** `GET /events?ledger=100&q=contract:CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB topic0:{"symbol":"mint"}`
  - URL-encoded. (CB has no mint event)
- **Expected:**
  - Status: 200
  - `data` length: 0

---

## Part 3: Test Coverage Mapping

### Existing tests to replace

| Existing Test                             | New q= Test                              |
|-------------------------------------------|------------------------------------------|
| `test_filter_by_type`                     | `test_q_filter_by_type`                  |
| `test_filter_by_contract_id`              | `test_q_filter_by_contract`              |
| `test_filter_by_topic_positional`         | `test_q_filter_by_topic`                 |
| `test_filter_topic_with_wildcard`         | `test_q_filter_topic_with_wildcard`      |
| `test_filters_or_logic`                   | `test_q_filter_or_logic`                 |
| `test_filters_and_logic_within`           | `test_q_filter_and_logic`                |
| `test_filters_combined_with_pagination`   | `test_q_filter_combined_with_pagination` |
| `test_filters_combined_with_ledger`       | `test_q_filter_combined_with_ledger`     |

### Existing tests to keep unchanged

These tests exercise the `filters` JSON parameter (backward compat) or unrelated features:
- `test_list_events_empty`
- `test_list_events_with_data`
- `test_pagination_with_before`
- `test_default_limit`
- `test_default_starts_at_latest_ledger`
- `test_ledger_filter`
- `test_ledger_filter_no_match`
- `test_filter_by_tx_hash`
- `test_tx_without_ledger_returns_error`
- `test_filter_topic_too_few_positions`
- `test_filters_all_wildcards`
- `test_filters_empty_array`
- `test_invalid_limit`
- `test_invalid_cursor`
- `test_filters_invalid_json`
- `test_filters_invalid_type`
- `test_error_response_format`
- `test_post_list_events`
- `test_post_with_filters`
- `test_post_invalid_limit`
- `test_post_empty_body`
- All `test_default_returns_descending_order`, `test_before_cursor_*`, `test_after_cursor_*`
- `test_list_envelope_consistency`
- `test_event_fields_complete`
- `test_status_endpoint`

---

## Part 4: Helper Functions Needed

### 4.1 `q_param(q: &str) -> String`

URL-encodes a `q` string value for use in GET query strings:

```rust
fn q_param(q: &str) -> String {
    urlencoding::encode(q).to_string()
}
```

Usage: `format!("{}/events?ledger=100&q={}", base_url, q_param(r#"type:contract topic0:{"symbol":"transfer"}"#))`

---

## Part 5: Implementation Notes

1. **Parser unit tests** should live in `src/api/query_parser.rs` as `#[cfg(test)] mod tests { ... }`, following the standard Rust convention. They test `parse_query()` directly without any HTTP layer.

2. **Integration tests** go in `tests/api_tests.rs` alongside the existing tests. They use the same `start_test_server` and `make_multi_type_events` helpers.

3. The `ListEventsRequest` struct needs a new `q: Option<String>` field for the POST body.

4. The `list_events_get` function needs to parse `q` from the query string multi-map and call `query_parser::parse_query()`.

5. When both `q` and `filters` are present, return `ApiError::BadRequest` before any parsing, with `param: Some("q")`.
