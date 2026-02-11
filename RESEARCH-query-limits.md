# Research: Query Complexity Limits for `q=` Filter Parameter

## 1. Findings from Production APIs

### GitHub Search API

GitHub's search API is the closest precedent to our `q=` parameter design.

- **Max query length**: 256 characters. Some older Enterprise versions enforce 128 characters.
- **Max boolean operators**: 5 AND/OR/NOT operators per query. This is a hard limit; queries with more than 5 boolean operators are rejected.
- **Max nesting depth**: Not explicitly documented; likely implicitly limited by the 5-operator cap.
- **Rate limiting**: 20 search requests/minute (authenticated), 5/minute (unauthenticated). This is separate from the general API rate limit.
- **Result cap**: 1,000 results total per search query.
- **No qualifier count limit documented** beyond the boolean operator cap and query length.

The 5-operator limit is notably strict. GitHub's approach is to keep individual queries simple and force users to split complex queries into multiple API calls.

### Elasticsearch

Elasticsearch is the industry standard for complex query DSLs and provides the most granular complexity controls.

- **Max boolean clauses** (`indices.query.bool.max_clause_count`): Default **4,096** (inherited from Lucene's default of **1,024**, raised in recent ES versions). Exceeding this throws `TooManyClausesException`.
- **Max nesting depth**: No explicit limit, but deep nesting amplifies clause count and memory usage. The clause count limit acts as an indirect cap.
- **Query string length**: No explicit default limit in Elasticsearch core. OpenSearch added `search.query.max_query_string_length` as a dynamic setting.
- **Expensive query control**: `search.allow_expensive_queries` (boolean) can disable regex, leading wildcards, and other costly patterns in production.
- **Result window**: `index.max_result_window` defaults to 10,000 (from + size).
- **Circuit breakers**: Memory-based circuit breakers abort queries that consume too much heap.

Elasticsearch's approach is primarily clause-count-based with memory circuit breakers as a backstop.

### Apache Lucene / Solr

Lucene is the underlying engine for Elasticsearch and Solr.

- **maxBooleanClauses**: Default **1,024** in Lucene and Solr. Configurable via system property `solr.max.booleanClauses`.
- **Behavior on exceed**: Throws `BooleanQuery$TooManyClauses` exception, rejecting the query.
- **Recommendation**: Keep at or below 1,024 unless heap/resources justify higher. Wildcard and prefix queries can silently expand to thousands of boolean clauses (e.g., a prefix query `foo*` on a field with 5,000 matching terms generates 5,000 boolean clauses).

### Stripe API Search

Stripe's search syntax is deliberately minimal to avoid complexity issues.

- **Max clauses**: **10** query clauses per search.
- **Boolean operators**: AND and OR supported, but **cannot mix AND and OR in the same query**. No parentheses for operator precedence.
- **No explicit query length limit** documented.
- **Substring matching**: Minimum 3 characters for fuzzy/substring operators.
- **Rate limit**: 20 read operations/second.

Stripe's approach is the most restrictive: no mixing of operators eliminates the need for nesting depth limits entirely.

### Datadog Log Search

- No documented query complexity limits for query length, terms/clauses, or nesting depth.
- Operational guidance recommends minimizing time ranges, using specific tags, avoiding broad searches and wildcards.
- Complexity is managed primarily through the UI and operational best practices rather than hard limits.

### Grafana Loki LogQL

- No documented query complexity limits for query length, label matchers, or pipeline stages.
- Loki's complexity is managed at the execution layer through configurable query timeouts, max query lookback periods, and per-tenant rate limits rather than syntactic complexity limits.

### GraphQL (General Pattern)

GraphQL APIs face similar complexity challenges and commonly use:

- **Query depth limits**: Typically 5-10 levels (Apollo Server, Hasura).
- **Complexity scoring**: Weighted cost per field/resolver. Common budget: 1,000 points. Scalars = 1 point, lists = 10x multiplier, nested objects = 5+.
- **Field count limits**: Some implementations cap total fields resolved.
- **Introspection disabling**: Production servers often disable schema introspection.

GraphQL's cost-based approach is sophisticated but designed for recursive/nested query languages, which is more complex than what we need.

## 2. Comparison Table

| System | Max Query Length | Max Clauses | Max Nesting Depth | Max Boolean Ops | Other Controls |
|---|---|---|---|---|---|
| **GitHub Search** | 256 chars | Not documented | Not documented | 5 AND/OR/NOT | 20 req/min rate limit |
| **Elasticsearch** | No default | 4,096 | No explicit limit | N/A (clause-based) | Circuit breakers, expensive query toggle |
| **Lucene/Solr** | No default | 1,024 | No explicit limit | N/A (clause-based) | Exception on exceed |
| **Stripe Search** | Not documented | 10 | N/A (no parens) | Cannot mix AND/OR | 20 read ops/sec |
| **Datadog** | Not documented | Not documented | Not documented | Not documented | Operational guidance |
| **Loki** | Not documented | Not documented | Not documented | Not documented | Execution timeouts |
| **GraphQL (typical)** | N/A | N/A | 5-10 | N/A | Cost scoring (~1,000 budget) |
| **OWASP Guidance** | 2,048-8,192 chars | 10-50 | 3-5 levels | N/A | Complexity scoring |

## 3. Concrete Recommendations for Our API

### Context-Specific Factors

Our use case has specific characteristics that inform the right limits:

1. **In-memory filtering**: Queries filter in-memory arrays, not a database. There is no query planner or index to abuse. The cost is linear in the number of events checked * the number of filter clauses.
2. **Small data per request**: Events are partitioned by ledger. A typical ledger has hundreds to low thousands of events. The dataset per query is bounded.
3. **OR expansion**: The `q=` parameter's boolean expression tree gets converted to a `Vec<EventFilter>` where each EventFilter represents one AND-combination and the Vec is OR'd. OR with nested groups can cause combinatorial expansion (e.g., `(A OR B) (C OR D)` = 4 EventFilters).
4. **Simple filter evaluation**: Each EventFilter check is a few string comparisons (contract ID, event type, topic values). This is cheap per-event.
5. **No wildcards or regex**: Our filters are exact-match only, eliminating the Lucene-style clause explosion problem.

### Recommended Limits

| Limit | Value | Rationale |
|---|---|---|
| **Max query string length** | **1,024 bytes** | Generous enough for real queries (a contract ID is 56 chars, a topic JSON value is ~30-80 chars). Well above GitHub's 256 but below OWASP's 8,192 ceiling. Prevents abuse from multi-KB query strings. |
| **Max number of terms/clauses** | **20** | A "term" is a single `key:value` pair in the query. 20 terms is generous for filtering events by contract, type, and a few topics. GitHub allows 5 operators; Stripe allows 10 clauses. 20 gives headroom for real queries while preventing abuse. |
| **Max nesting depth** | **4 levels** | Parentheses nesting depth. Supports `(A OR (B AND C))` patterns. Deeper nesting provides no practical value for event filtering. OWASP recommends 3-5. |
| **Max OR branches (after expansion)** | **20 EventFilters** | This is the critical limit. When the boolean expression is converted to disjunctive normal form (OR of ANDs), the number of resulting EventFilter objects should not exceed 20. This prevents combinatorial explosion from patterns like `(A OR B)(C OR D)(E OR F)` which would produce 8 filters. 20 is generous for any real query pattern. |
| **Max topics per filter** | **4** | Stellar contract events have at most 4 topics. No point allowing more. |

### Why These Numbers

**1,024 bytes for query length**: A realistic complex query looks like:
```
(contract:CABC...56chars... OR contract:CXYZ...56chars...) type:contract topic0:{"symbol":"transfer"} topic2:{"address":"GABC...56chars..."}
```
This is roughly 250 characters. Doubling that for safety and adding room for more OR branches gives ~500-600 chars. 1,024 bytes provides ample headroom while still being bounded.

**20 terms**: The most complex realistic query involves filtering by multiple contracts (say 5-10 via OR), with type and 2-3 topic positions. That is roughly 15 terms. 20 provides headroom without enabling abuse.

**4 nesting levels**: Real queries rarely need more than 2 levels: one level to group OR'd contracts, one level for the overall query. 4 is conservative.

**20 EventFilters after expansion**: This is the most important limit because it directly controls the work done per event. With 20 filters and ~1,000 events per ledger, the worst case is 20,000 filter checks per query — each check being a few string comparisons. This is trivially fast (microseconds). But allowing 100+ filters could start to matter, and 1,000+ filters (from combinatorial explosion) would be wasteful.

## 4. Implementation Recommendations

### Where to Check: Parse Time (Fast Fail)

All limits should be checked at parse time, before any event filtering occurs. This follows the principle of failing fast and returning clear error messages.

```
Request arrives
  -> Check query string byte length (< 1,024)
  -> Parse query string into AST
  -> Check AST term count (< 20)
  -> Check AST nesting depth (< 4)
  -> Convert AST to Vec<EventFilter> (DNF expansion)
  -> Check expanded filter count (< 20)
  -> Execute query against events
```

### Specific Check Points

1. **Query string length**: Check immediately upon receiving the `q=` parameter, before any parsing. This is the cheapest check and catches the most obvious abuse.

2. **Term count**: Count during parsing. Each `key:value` pair increments the counter. Reject if > 20 before completing the parse.

3. **Nesting depth**: Track during parsing. Each opening parenthesis increments depth; each closing one decrements. Reject if depth exceeds 4 at any point.

4. **Expanded filter count**: Check after converting the boolean expression to disjunctive normal form. This catches combinatorial explosion that individual term/depth limits might miss.

### Error Messages

Follow the existing API error format. Use `invalid_request_error` type with `query_too_complex` code and a descriptive message.

```json
{
  "error": {
    "type": "invalid_request_error",
    "code": "query_too_complex",
    "message": "query exceeds maximum length of 1024 bytes",
    "param": "q"
  }
}
```

```json
{
  "error": {
    "type": "invalid_request_error",
    "code": "query_too_complex",
    "message": "query contains 25 terms, maximum is 20",
    "param": "q"
  }
}
```

```json
{
  "error": {
    "type": "invalid_request_error",
    "code": "query_too_complex",
    "message": "query nesting depth of 6 exceeds maximum of 4",
    "param": "q"
  }
}
```

```json
{
  "error": {
    "type": "invalid_request_error",
    "code": "query_too_complex",
    "message": "query expands to 64 filter combinations, maximum is 20",
    "param": "q"
  }
}
```

### Constants

Define limits as named constants for easy tuning:

```rust
/// Maximum byte length of the `q` query parameter.
const MAX_QUERY_LENGTH: usize = 1024;

/// Maximum number of key:value terms in a query.
const MAX_QUERY_TERMS: usize = 20;

/// Maximum parenthesis nesting depth.
const MAX_QUERY_DEPTH: usize = 4;

/// Maximum number of EventFilter objects after boolean expansion.
const MAX_EXPANDED_FILTERS: usize = 20;
```

### What NOT to Do

- **Do not use execution-time timeouts as the primary guard**. Timeouts are a last resort; parse-time validation is cheaper and gives better error messages.
- **Do not use cost-based scoring**. Our query language is simple enough that counting terms and filters is sufficient. Cost scoring adds implementation complexity without proportional benefit.
- **Do not limit at the HTTP layer** (e.g., max URL length in the reverse proxy). HTTP-layer limits are too coarse and produce opaque errors. Validate in application code.
- **Do not make limits configurable per-user**. Start with fixed limits. Add configurability only if real users need higher limits, which is unlikely given the generous defaults.
