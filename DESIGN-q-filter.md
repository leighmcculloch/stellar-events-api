# Design: `q=` Filter Query Syntax

## Overview

Replace the JSON `filters` parameter with a `q` query parameter using a GitHub search-style syntax. The `q` parameter supports key:value qualifiers, implicit AND, explicit OR, and parenthesized grouping.

## 1. Syntax Specification

### Grammar (PEG-style)

```
query       = ws expr ws
expr        = or_expr
or_expr     = and_expr (WS "OR" WS and_expr)*
and_expr    = atom (WS atom)*
atom        = "(" ws expr ws ")" / qualifier
qualifier   = key ":" value
key         = "type" / "contract" / "topic" / "topic0" / "topic1" / "topic2" / "topic3"
value       = quoted_value / json_value / bare_value
quoted_value = '"' ( '\\"' / [^"] )* '"'
json_value  = '{' json_inner '}'
json_inner  = ( '{' json_inner '}' / '"' ( '\\"' / [^"] )* '"' / [^{}] )*
bare_value  = [^ \t()"]+
ws          = [ \t]*
WS          = [ \t]+
```

### Tokenizer Tokens

| Token         | Pattern                               | Example                          |
|---------------|---------------------------------------|----------------------------------|
| `QUALIFIER`   | `key:value`                           | `type:contract`                  |
| `OR`          | literal `OR` surrounded by whitespace | `OR`                             |
| `LPAREN`      | `(`                                   | `(`                              |
| `RPAREN`      | `)`                                   | `)`                              |

The tokenizer must handle JSON brace-balancing when reading the value portion of a qualifier. When a `:` is followed by `{`, the tokenizer reads until the matching `}`, tracking brace depth.

### Key Observations

- AND is implicit (space-separated qualifiers).
- OR must be explicit and uppercased.
- AND binds tighter than OR: `a b OR c d` means `(a AND b) OR (c AND d)`.
- Parentheses override precedence.
- There are no bare search terms (every token must be a `key:value` qualifier, `OR`, or a parenthesis).

## 2. Key Definitions

| Key         | Value Format                    | Maps To                       | Description                                       |
|-------------|---------------------------------|-------------------------------|---------------------------------------------------|
| `type`      | `contract`, `system`, `diagnostic` | `EventFilter.event_type`   | Event type. Case-sensitive.                       |
| `contract`  | Stellar strkey (starts with `C`) | `EventFilter.contract_id`   | Contract ID.                                      |
| `topic`     | XDR-JSON ScVal                  | `EventFilter.any_topics`     | Matches if the value appears in any topic position. Multiple `topic` qualifiers in one AND-group are AND'd (all must be present). |
| `topic0`    | XDR-JSON ScVal                  | `EventFilter.topics[0]`      | Exact match on the first topic position.          |
| `topic1`    | XDR-JSON ScVal                  | `EventFilter.topics[1]`      | Exact match on the second topic position.         |
| `topic2`    | XDR-JSON ScVal                  | `EventFilter.topics[2]`      | Exact match on the third topic position.          |
| `topic3`    | XDR-JSON ScVal                  | `EventFilter.topics[3]`      | Exact match on the fourth topic position.         |

### Value Formats

- **`type`**: One of the string literals `contract`, `system`, `diagnostic`. No quoting needed.
- **`contract`**: A bare Stellar strkey string (e.g., `CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAFCT4`). No quoting needed since strkeys contain no spaces or special characters.
- **`topic0`..`topic3`**: An XDR-JSON ScVal object, always starting with `{` and ending with `}`. The tokenizer reads balanced braces. Examples:
  - `topic0:{"symbol":"transfer"}`
  - `topic1:{"address":"GDEF..."}`

Quoted values (double quotes) are supported for any key if needed, but in practice only required if a value contains spaces. The quotes are stripped before interpretation.

## 3. Boolean Logic

### Operator Precedence (highest to lowest)

1. **Parentheses** `(...)` -- explicit grouping
2. **AND** (implicit) -- space-separated qualifiers
3. **OR** (explicit) -- the literal keyword `OR`

### Semantics

The parsed expression tree is a boolean formula over individual qualifiers. Each qualifier represents a single field constraint on an event. The expression is evaluated as:

- **AND**: An event matches an AND group if it matches every qualifier in the group.
- **OR**: An event matches an OR expression if it matches at least one operand.

### Examples

| Query                                              | Interpretation                                                   |
|----------------------------------------------------|------------------------------------------------------------------|
| `type:contract`                                    | type=contract                                                    |
| `type:contract contract:CA...`                     | type=contract AND contract=CA...                                 |
| `type:contract OR type:system`                     | type=contract OR type=system                                     |
| `(contract:CA... OR contract:CB...) topic0:{"symbol":"transfer"}` | (contract=CA... OR contract=CB...) AND topic0=transfer  |
| `type:contract topic0:{"symbol":"transfer"} OR type:system topic0:{"symbol":"mint"}` | (type=contract AND topic0=transfer) OR (type=system AND topic0=mint) |

## 4. Parsing Strategy

### Phase 1: Tokenization

A hand-written tokenizer that scans the input string left-to-right and emits a stream of tokens:

```
Input: (contract:CA... OR contract:CB...) topic0:{"symbol":"transfer"}
Tokens:
  LPAREN
  QUALIFIER("contract", "CA...")
  OR
  QUALIFIER("contract", "CB...")
  RPAREN
  QUALIFIER("topic0", "{\"symbol\":\"transfer\"}")
```

**Tokenizer rules:**

1. Skip whitespace between tokens.
2. `(` emits `LPAREN`.
3. `)` emits `RPAREN`.
4. `OR` followed by whitespace or `)` or end-of-input emits `OR`.
5. Otherwise, read a qualifier: scan for `key:value`. The key is characters up to the first `:`. The value is:
   - If starts with `"`: read until unescaped closing `"`.
   - If starts with `{`: read with brace-depth tracking until the matching `}`.
   - Otherwise: read until whitespace, `)`, or end-of-input.
6. Unknown key names produce an error token (deferred to parsing phase).

### Phase 2: Parse to AST

A recursive descent parser that consumes the token stream and builds an AST:

```rust
enum QueryExpr {
    Qualifier { key: String, value: String },
    And(Vec<QueryExpr>),
    Or(Vec<QueryExpr>),
}
```

**Grammar implementation:**

```
parse_or()  -> parse_and() ("OR" parse_and())*
parse_and() -> parse_atom() (parse_atom())*   // implicit AND
parse_atom() -> "(" parse_or() ")" | QUALIFIER
```

### Phase 3: AST to `Vec<EventFilter>`

Convert the parsed AST into the existing `Vec<EventFilter>` structure using the conversion rules below.

## 5. Conversion Rules: AST to `Vec<EventFilter>`

The existing system interprets `Vec<EventFilter>` as: filters are OR'd together, and conditions within a single filter are AND'd. This is equivalent to a boolean formula in Disjunctive Normal Form (DNF): `(A AND B) OR (C AND D) OR ...`.

The conversion normalizes the AST into DNF, then maps each conjunction (AND-group) to one `EventFilter`.

### Algorithm

1. **Normalize to DNF.** Distribute AND over OR to produce a flat `OR(AND(...), AND(...), ...)` structure.

   Example: `(A OR B) AND C` becomes `(A AND C) OR (B AND C)`.

2. **Map each AND-group to an `EventFilter`.** For each conjunction:
   - Collect all `type:V` qualifiers: if present, set `event_type = Some(V)`. Error if multiple conflicting `type` values in one AND-group.
   - Collect all `contract:V` qualifiers: if present, set `contract_id = Some(V)`. Error if multiple conflicting `contract` values in one AND-group.
   - Collect all `topicN:V` qualifiers: build the `topics` vector. The vector length is `max(N) + 1`. Unspecified positions are filled with `null` (wildcard). Error if the same `topicN` appears twice in one AND-group.

3. **Assemble `Vec<EventFilter>`.** Each AND-group becomes one `EventFilter`. The resulting vector represents the top-level OR.

### Conversion Examples

**Query:** `type:contract topic0:{"symbol":"transfer"}`

DNF: one conjunction with two qualifiers.

```rust
vec![EventFilter {
    event_type: Some("contract".into()),
    contract_id: None,
    topics: Some(vec![json!({"symbol": "transfer"})]),
}]
```

**Query:** `(contract:CA... OR contract:CB...) topic0:{"symbol":"transfer"}`

DNF after distribution:
- `contract:CA... AND topic0:{"symbol":"transfer"}`
- `contract:CB... AND topic0:{"symbol":"transfer"}`

```rust
vec![
    EventFilter {
        contract_id: Some("CA...".into()),
        event_type: None,
        topics: Some(vec![json!({"symbol": "transfer"})]),
    },
    EventFilter {
        contract_id: Some("CB...".into()),
        event_type: None,
        topics: Some(vec![json!({"symbol": "transfer"})]),
    },
]
```

**Query:** `topic0:{"symbol":"transfer"} topic2:{"address":"GDEF..."}`

Topics vector fills gaps with null:

```rust
vec![EventFilter {
    contract_id: None,
    event_type: None,
    topics: Some(vec![
        json!({"symbol": "transfer"}),
        serde_json::Value::Null,
        json!({"address": "GDEF..."}),
    ]),
}]
```

### DNF Expansion Limit

DNF expansion can cause combinatorial blowup (e.g., `(A OR B) (C OR D) (E OR F)` produces 8 conjunctions). To prevent abuse:

- **Hard limit:** The resulting `Vec<EventFilter>` must contain no more than **20 filters** after DNF expansion. If exceeded, return an error.
- This mirrors the existing implicit limit on how many filter objects a user could pass in the JSON `filters` array.

## 6. Error Handling

The parser returns structured errors that map to `ApiError::BadRequest` with `param: Some("q")`.

### Error Types

| Error                      | Condition                                                       | Example                                |
|----------------------------|-----------------------------------------------------------------|----------------------------------------|
| `empty_query`              | `q` is empty or whitespace-only                                 | `q=`                                   |
| `unknown_key`              | Qualifier uses an unrecognized key                              | `q=foo:bar`                            |
| `missing_value`            | Qualifier has no value after `:`                                 | `q=type:`                              |
| `invalid_value`            | Value doesn't match the key's expected format                   | `q=type:invalid`                       |
| `unbalanced_parens`        | Mismatched parentheses                                          | `q=(type:contract`                     |
| `unexpected_token`         | Token in wrong position (e.g., `OR` at start, two ORs in a row)| `q=OR type:contract`                   |
| `conflicting_qualifiers`   | Same key repeated with different values in one AND-group        | `q=type:contract type:system`          |
| `duplicate_topic_position` | Same topicN key repeated in one AND-group                       | `q=topic0:{"symbol":"a"} topic0:{"symbol":"b"}` |
| `unbalanced_braces`        | JSON brace `{` without matching `}`                             | `q=topic0:{"symbol":"transfer"`        |
| `unbalanced_quotes`        | Opening `"` without closing `"`                                 | `q=type:"contract`                     |
| `too_many_filters`         | DNF expansion exceeds the 20-filter limit                       | deeply nested OR expressions           |

### Error Message Format

```json
{
  "error": {
    "type": "invalid_request_error",
    "code": "invalid_parameter",
    "message": "invalid q parameter: unknown key 'foo' (expected: type, contract, topic0, topic1, topic2, topic3)",
    "param": "q"
  }
}
```

## 7. Edge Cases

| Input                                           | Behavior                                                              |
|-------------------------------------------------|-----------------------------------------------------------------------|
| `q=` (empty)                                    | Error: `empty_query`                                                  |
| `q=   ` (whitespace only)                       | Error: `empty_query`                                                  |
| No `q` param at all                             | No filters applied (return all events). Same as current no-filter behavior. |
| `q=type:contract type:contract`                 | Valid: duplicate qualifiers with the same value are collapsed.        |
| `q=type:contract type:system`                   | Error: `conflicting_qualifiers` -- cannot be both types in one AND-group. |
| `q=(type:contract OR type:system)`              | Valid: produces two filters, one per type.                            |
| `q=()` (empty parens)                           | Error: `unexpected_token` (`)` after `(`).                            |
| `q=topic0:{"symbol":"transfer"}` (JSON value)   | Valid: braces are balanced, parsed as JSON string value.             |
| `q=topic0:{"nested":{"a":"b"}}` (nested JSON)   | Valid: brace balancing handles nesting.                              |
| `q=type:CONTRACT` (wrong case)                  | Error: `invalid_value` -- values are case-sensitive.                 |
| `q=type:contract OR` (trailing OR)              | Error: `unexpected_token` -- OR must be followed by an expression.   |
| `q=OR type:contract` (leading OR)               | Error: `unexpected_token` -- OR must be preceded by an expression.   |
| `q=type:contract OR OR type:system`             | Error: `unexpected_token` -- consecutive OR operators.               |
| Extra whitespace `q=  type:contract  `          | Valid: leading/trailing whitespace is trimmed.                       |
| `q=contract:CA... contract:CB...`               | Error: `conflicting_qualifiers` -- use OR for multiple contracts.    |

## 8. URL Encoding Notes

Since the `q` value is a URL query parameter, characters like `{`, `}`, `"`, and spaces must be percent-encoded by the client:

```
/events?q=type:contract%20topic0:%7B%22symbol%22:%22transfer%22%7D
```

Decodes to:

```
type:contract topic0:{"symbol":"transfer"}
```

The server URL-decodes the `q` parameter value before passing it to the parser. This is already handled by the existing `parse_multi_params` function via `urlencoding::decode`.

## 9. Backward Compatibility

- The `filters` JSON parameter continues to work during a transition period.
- If both `filters` and `q` are provided, return an error: `"filters and q cannot both be provided"`.
- The POST JSON body gains an optional `q` field of type `String`, with the same parsing rules.

## 10. Implementation Modules

The parser should live in a new module: `src/api/query_parser.rs`.

### Public Interface

```rust
/// Parse a q= filter string into a Vec<EventFilter>.
pub fn parse_query(input: &str) -> Result<Vec<EventFilter>, QueryParseError>;

/// Structured parse error with position information.
pub struct QueryParseError {
    pub kind: QueryParseErrorKind,
    pub message: String,
    pub position: usize,  // byte offset in input
}
```

### Integration Point

In `src/api/routes.rs`, the `list_events_get` function adds:

```rust
let filters: Vec<EventFilter> = match (multi.get("q"), multi.get("filters")) {
    (Some(q), Some(_)) => return Err(ApiError::BadRequest {
        message: "filters and q cannot both be provided".into(),
        param: Some("q".into()),
    }),
    (Some(q_values), None) => {
        let q = q_values.first().unwrap();
        query_parser::parse_query(q).map_err(|e| ApiError::BadRequest {
            message: format!("invalid q parameter: {}", e.message),
            param: Some("q".into()),
        })?
    },
    (None, Some(filter_values)) => { /* existing JSON parsing */ },
    (None, None) => Vec::new(),
};
```
