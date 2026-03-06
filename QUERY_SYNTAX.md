# Query Syntax

The `q` parameter filters events using a search-style query language. Queries
are built from `key:value` qualifiers combined with boolean logic.

## Qualifiers

Each qualifier is a `key:value` pair. The key determines which event field to
match and the value specifies the expected content.

| Key | Value | Description |
|-----|-------|-------------|
| `type` | `contract`, `system`, or `diagnostic` | Match events of this type. Case-sensitive. |
| `contract` | Stellar contract strkey (C…) | Match events emitted by this contract. |
| `ledger` | Positive integer | Match events from this ledger sequence number. |
| `tx` | Transaction hash hex string | Match events from this transaction. Requires `ledger`. |
| `topic0` … `topic3` | XDR-JSON `ScVal` object | Match events whose topic at position 0–3 equals this value exactly. |
| `topic` | XDR-JSON `ScVal` object | Match events that contain this value in **any** topic position. |

### Topic values

Topic values are XDR-JSON `ScVal` objects, not plain strings. They must be
valid JSON objects wrapped in braces:

```
topic0:{"symbol":"transfer"}
topic1:{"address":"GABC..."}
topic2:{"i128":{"hi":0,"lo":1000000}}
```

If a value contains spaces, wrap it in double quotes:

```
topic0:"{"symbol":"transfer"}"
```

## Boolean logic

### AND (implicit)

Space-separated qualifiers are AND'd. All conditions must match:

```
type:contract contract:CCW67...
```

This matches events that are **both** type `contract` **and** from contract
`CCW67...`.

### OR (explicit)

Use the `OR` keyword (uppercase) between expressions:

```
type:contract OR type:system
```

This matches events that are **either** type `contract` **or** type `system`.

### Precedence

AND binds tighter than OR. This query:

```
type:contract topic0:{"symbol":"transfer"} OR type:system topic0:{"symbol":"core_metrics"}
```

is interpreted as:

```
(type:contract AND topic0:{"symbol":"transfer"}) OR (type:system AND topic0:{"symbol":"core_metrics"})
```

### Parentheses

Use parentheses to override precedence or clarify grouping:

```
(type:contract OR type:system) contract:CCW67...
```

This matches events from contract `CCW67...` that are either type `contract`
or type `system`. Without parentheses, the `OR` would split the query into two
independent alternatives.

Parentheses can be nested up to 4 levels deep.

## Positional topic matching

Use `topic0` through `topic3` to match specific topic positions. When multiple
positions are specified, gaps are filled with wildcards:

```
topic0:{"symbol":"transfer"} topic2:{"address":"GABC..."}
```

This matches events where:
- Topic position 0 equals `{"symbol":"transfer"}`
- Topic position 1 can be anything (wildcard)
- Topic position 2 equals `{"address":"GABC..."}`

The event must have at least as many topics as the highest specified position
(3 in this example).

## Any-position topic matching

Use `topic` (without a position number) to match a value in any topic
position. The event matches if the value appears anywhere in its topic list:

```
topic:{"address":"GABC..."}
```

Multiple `topic` qualifiers in the same filter are AND'd — all values must
appear somewhere in the event's topics, though they can be in any positions:

```
topic:{"symbol":"transfer"} topic:{"address":"GABC..."}
```

`topic` and positional `topic0`–`topic3` can be combined in the same filter.
Both constraints must be satisfied independently.

## Conflicting qualifiers

Within one filter group (one side of an OR), a key that accepts a single value
cannot be specified twice with different values:

```
type:contract type:system          ← ERROR: conflicting types
contract:CABC... contract:CDEF...  ← ERROR: conflicting contracts
ledger:100 ledger:200              ← ERROR: conflicting ledgers
```

Use `OR` to match multiple values:

```
type:contract OR type:system
```

Duplicate qualifiers with the **same** value are silently collapsed:

```
type:contract type:contract        ← OK, treated as single type:contract
```

## DNF expansion

Queries with mixed AND and OR are internally expanded into Disjunctive Normal
Form (OR of ANDs). Each resulting AND-group becomes a separate filter. Filters
are matched independently against each event; an event matches if **any**
filter matches.

For example:

```
(contract:CABC... OR contract:CDEF...) (topic0:{"symbol":"transfer"} OR topic0:{"symbol":"mint"})
```

expands into 4 filters (2 × 2 cartesian product):

1. `contract:CABC... AND topic0:{"symbol":"transfer"}`
2. `contract:CABC... AND topic0:{"symbol":"mint"}`
3. `contract:CDEF... AND topic0:{"symbol":"transfer"}`
4. `contract:CDEF... AND topic0:{"symbol":"mint"}`

## Limits

| Limit | Value |
|-------|-------|
| Maximum query length | 1,024 bytes |
| Maximum terms (key:value pairs) | 20 |
| Maximum parenthesis nesting depth | 4 |
| Maximum filter combinations after DNF expansion | 20 |

Queries that exceed these limits return a `400 Bad Request` error.

## Examples

**All contract events from a specific ledger:**
```
ledger:58000000 type:contract
```

**Transfer events for a contract:**
```
contract:CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75 topic0:{"symbol":"transfer"}
```

**Events from a specific transaction:**
```
ledger:58000000 tx:7758a34695323011e177c932cb899f3ea55c5af4d95c954e946ddddaafca0296
```

**Transfer, mint, clawback, and burn events for XLM and USDC:**
```
(contract:CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA OR contract:CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75) (topic0:{"symbol":"transfer"} OR topic0:{"symbol":"mint"} OR topic0:{"symbol":"clawback"} OR topic0:{"symbol":"burn"})
```

**Events where a specific address appears in any topic position:**
```
contract:CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75 topic:{"address":"GABC..."}
```

**Contract or system events (either type):**
```
type:contract OR type:system
```
