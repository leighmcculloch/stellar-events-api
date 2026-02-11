# JSON Query Syntax

The `q` parameter in POST requests can be a structured JSON object instead of
a [string query](QUERY_SYNTAX.md). Both formats express the same filtering
logic and are fully interconvertible. A [JSON Schema](/schema) describing the
format is served by the API.

## Node types

Every node is a JSON object with exactly one key. There are three kinds of
nodes:

### Qualifier

A single filter condition. The key is one of the qualifier keys listed below
and the value depends on the key.

```json
{"type": "contract"}
{"ledger": 58000000}
{"topic0": {"symbol": "transfer"}}
```

### And

All children must match. The `"and"` key maps to an array of one or more
child nodes.

```json
{"and": [{"type": "contract"}, {"contract": "CCW67..."}]}
```

### Or

At least one child must match. The `"or"` key maps to an array of one or more
child nodes.

```json
{"or": [{"type": "contract"}, {"type": "system"}]}
```

Nodes can be nested arbitrarily (up to 4 levels deep):

```json
{
  "and": [
    {"or": [{"type": "contract"}, {"type": "system"}]},
    {"contract": "CCW67..."}
  ]
}
```

**Important:** Each JSON object must have exactly one key. To combine multiple
qualifiers, wrap them in `"and"`:

```json
{"type": "contract", "contract": "CCW67..."}
```

This is **invalid** — use this instead:

```json
{"and": [{"type": "contract"}, {"contract": "CCW67..."}]}
```

## Qualifier keys

| Key | JSON value type | Description |
|-----|-----------------|-------------|
| `type` | string: `"contract"`, `"system"`, or `"diagnostic"` | Match events of this type. |
| `contract` | string (Stellar contract strkey C…) | Match events emitted by this contract. |
| `ledger` | integer (positive) | Match events from this ledger sequence number. |
| `tx` | string (transaction hash hex) | Match events from this transaction. Requires `ledger` in the same filter. |
| `topic0` … `topic3` | any JSON value (XDR-JSON `ScVal`) | Match events whose topic at position 0–3 equals this value exactly. |
| `topic` | any JSON value (XDR-JSON `ScVal`) | Match events that contain this value in **any** topic position. |

### Topic values

Topic values are XDR-JSON `ScVal` objects:

```json
{"topic0": {"symbol": "transfer"}}
{"topic1": {"address": "GABC..."}}
{"topic2": {"i128": {"hi": 0, "lo": 1000000}}}
```

## Mapping to string syntax

The JSON format maps 1:1 to the [string query syntax](QUERY_SYNTAX.md):

| String | JSON |
|--------|------|
| `type:contract` | `{"type":"contract"}` |
| `type:contract contract:CCW67...` | `{"and":[{"type":"contract"},{"contract":"CCW67..."}]}` |
| `type:contract OR type:system` | `{"or":[{"type":"contract"},{"type":"system"}]}` |
| `(type:contract OR type:system) contract:CCW67...` | `{"and":[{"or":[{"type":"contract"},{"type":"system"}]},{"contract":"CCW67..."}]}` |
| `topic0:{"symbol":"transfer"}` | `{"topic0":{"symbol":"transfer"}}` |
| `ledger:100 tx:abc` | `{"and":[{"ledger":100},{"tx":"abc"}]}` |

## Full POST request examples

**Filter by event type:**

```json
{
  "limit": 10,
  "q": {"type": "contract"}
}
```

**AND — type and contract:**

```json
{
  "limit": 10,
  "q": {
    "and": [
      {"type": "contract"},
      {"contract": "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75"}
    ]
  }
}
```

**OR — multiple types:**

```json
{
  "limit": 10,
  "q": {
    "or": [
      {"type": "contract"},
      {"type": "system"}
    ]
  }
}
```

**Nested — (contract OR system) AND specific contract AND topic:**

```json
{
  "limit": 10,
  "q": {
    "and": [
      {"or": [{"type": "contract"}, {"type": "system"}]},
      {"contract": "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75"},
      {"topic0": {"symbol": "transfer"}}
    ]
  }
}
```

**Transfer, mint, clawback, and burn events for XLM and USDC:**

```json
{
  "limit": 10,
  "q": {
    "and": [
      {
        "or": [
          {"contract": "CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA"},
          {"contract": "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75"}
        ]
      },
      {
        "or": [
          {"topic0": {"symbol": "transfer"}},
          {"topic0": {"symbol": "mint"}},
          {"topic0": {"symbol": "clawback"}},
          {"topic0": {"symbol": "burn"}}
        ]
      }
    ]
  }
}
```

**Events from a specific transaction:**

```json
{
  "q": {
    "and": [
      {"ledger": 58000000},
      {"tx": "7758a34695323011e177c932cb899f3ea55c5af4d95c954e946ddddaafca0296"}
    ]
  }
}
```

**Events where a specific address appears in any topic position:**

```json
{
  "q": {
    "and": [
      {"contract": "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75"},
      {"topic": {"address": "GABC..."}}
    ]
  }
}
```

## Positional topic matching

Use `topic0` through `topic3` to match specific topic positions. When multiple
positions are specified in an `"and"`, gaps act as wildcards:

```json
{
  "q": {
    "and": [
      {"topic0": {"symbol": "transfer"}},
      {"topic2": {"address": "GABC..."}}
    ]
  }
}
```

This matches events where topic 0 equals `{"symbol":"transfer"}`, topic 1 can
be anything, and topic 2 equals `{"address":"GABC..."}`.

## Any-position topic matching

Use `topic` (without a position number) to match a value in any topic
position:

```json
{"topic": {"address": "GABC..."}}
```

Multiple `topic` qualifiers in an `"and"` are all required — every value must
appear somewhere in the event's topics:

```json
{
  "q": {
    "and": [
      {"topic": {"symbol": "transfer"}},
      {"topic": {"address": "GABC..."}}
    ]
  }
}
```

## DNF expansion

Queries with mixed `and` and `or` are internally expanded into Disjunctive
Normal Form (OR of ANDs). Each resulting AND-group becomes a separate filter.

For example:

```json
{
  "q": {
    "and": [
      {"or": [{"contract": "CABC..."}, {"contract": "CDEF..."}]},
      {"or": [{"topic0": {"symbol": "transfer"}}, {"topic0": {"symbol": "mint"}}]}
    ]
  }
}
```

expands into 4 filters (2 × 2 cartesian product):

1. `contract:CABC... AND topic0:transfer`
2. `contract:CABC... AND topic0:mint`
3. `contract:CDEF... AND topic0:transfer`
4. `contract:CDEF... AND topic0:mint`

## Limits

| Limit | Value |
|-------|-------|
| Maximum terms (qualifier nodes) | 20 |
| Maximum nesting depth | 4 |
| Maximum filter combinations after DNF expansion | 20 |

Queries that exceed these limits return a `400 Bad Request` error.
