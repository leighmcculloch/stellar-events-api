use std::fmt;

use crate::db::EventFilter;
use crate::ledger::events::EventType;

/// Structured parse error with position information.
#[derive(Debug)]
pub struct QueryParseError {
    pub kind: QueryParseErrorKind,
    pub message: String,
    pub position: usize,
}

impl fmt::Display for QueryParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum QueryParseErrorKind {
    EmptyQuery,
    UnknownKey,
    MissingValue,
    InvalidValue,
    UnbalancedParens,
    UnexpectedToken,
    ConflictingQualifiers,
    DuplicateTopicPosition,
    UnbalancedBraces,
    UnbalancedQuotes,
    TooManyFilters,
    QueryTooLong,
    TooManyTerms,
    NestingTooDeep,
}

/// Maximum byte length of the `q` query parameter.
const MAX_QUERY_LENGTH: usize = 1024;

/// Maximum number of key:value terms in a query.
const MAX_QUERY_TERMS: usize = 20;

/// Maximum parenthesis nesting depth.
const MAX_NESTING_DEPTH: usize = 4;

/// Maximum number of EventFilter objects after boolean expansion.
const MAX_FILTERS: usize = 20;

/// Parse a q= filter string into a Vec<EventFilter>.
pub fn parse_query(input: &str) -> Result<Vec<EventFilter>, QueryParseError> {
    if input.len() > MAX_QUERY_LENGTH {
        return Err(QueryParseError {
            kind: QueryParseErrorKind::QueryTooLong,
            message: format!("query exceeds maximum length of {} bytes", MAX_QUERY_LENGTH),
            position: 0,
        });
    }

    if input.trim().is_empty() {
        return Err(QueryParseError {
            kind: QueryParseErrorKind::EmptyQuery,
            message: "query is empty".to_string(),
            position: 0,
        });
    }

    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err(QueryParseError {
            kind: QueryParseErrorKind::EmptyQuery,
            message: "query is empty".to_string(),
            position: 0,
        });
    }

    let term_count = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Qualifier)
        .count();
    if term_count > MAX_QUERY_TERMS {
        return Err(QueryParseError {
            kind: QueryParseErrorKind::TooManyTerms,
            message: format!("query exceeds maximum of {} terms", MAX_QUERY_TERMS),
            position: 0,
        });
    }

    let mut parser = Parser::new(&tokens);
    let expr = parser.parse_or()?;

    if parser.pos < parser.tokens.len() {
        let tok = &parser.tokens[parser.pos];
        return Err(QueryParseError {
            kind: QueryParseErrorKind::UnexpectedToken,
            message: format!("unexpected token '{}'", tok.text),
            position: tok.position,
        });
    }

    let dnf = to_dnf(expr);
    let and_groups = flatten_or(dnf);

    if and_groups.len() > MAX_FILTERS {
        return Err(QueryParseError {
            kind: QueryParseErrorKind::TooManyFilters,
            message: format!(
                "query expands to {} filters, maximum is {}",
                and_groups.len(),
                MAX_FILTERS,
            ),
            position: 0,
        });
    }

    let mut filters = Vec::with_capacity(and_groups.len());
    for group in and_groups {
        filters.push(and_group_to_filter(group)?);
    }

    Ok(filters)
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum TokenKind {
    Qualifier,
    Or,
    LParen,
    RParen,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    text: String,
    key: Option<String>,
    value: Option<String>,
    position: usize,
}

const VALID_KEYS: &[&str] = &[
    "type", "contract", "topic", "topic0", "topic1", "topic2", "topic3", "ledger", "tx",
];

fn tokenize(input: &str) -> Result<Vec<Token>, QueryParseError> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut pos = 0;
    let mut tokens = Vec::new();

    while pos < len {
        // Skip whitespace.
        if bytes[pos] == b' ' || bytes[pos] == b'\t' {
            pos += 1;
            continue;
        }

        let start = pos;

        // LPAREN
        if bytes[pos] == b'(' {
            tokens.push(Token {
                kind: TokenKind::LParen,
                text: "(".to_string(),
                key: None,
                value: None,
                position: start,
            });
            pos += 1;
            continue;
        }

        // RPAREN
        if bytes[pos] == b')' {
            tokens.push(Token {
                kind: TokenKind::RParen,
                text: ")".to_string(),
                key: None,
                value: None,
                position: start,
            });
            pos += 1;
            continue;
        }

        // Check for OR keyword: must be "OR" followed by whitespace, ), or end.
        if pos + 2 <= len
            && &input[pos..pos + 2] == "OR"
            && (pos + 2 == len
                || bytes[pos + 2] == b' '
                || bytes[pos + 2] == b'\t'
                || bytes[pos + 2] == b')')
        {
            tokens.push(Token {
                kind: TokenKind::Or,
                text: "OR".to_string(),
                key: None,
                value: None,
                position: start,
            });
            pos += 2;
            continue;
        }

        // Otherwise, read a qualifier: key:value
        // Scan for key (everything up to ':')
        let key_start = pos;
        while pos < len
            && bytes[pos] != b':'
            && bytes[pos] != b' '
            && bytes[pos] != b'\t'
            && bytes[pos] != b'('
            && bytes[pos] != b')'
        {
            pos += 1;
        }

        if pos >= len || bytes[pos] != b':' {
            let word = &input[key_start..pos];
            return Err(QueryParseError {
                kind: QueryParseErrorKind::UnexpectedToken,
                message: format!(
                    "unexpected token '{}' (expected key:value qualifier, OR, or parenthesis)",
                    word
                ),
                position: key_start,
            });
        }

        let key = &input[key_start..pos];

        // Validate key.
        if !VALID_KEYS.contains(&key) {
            return Err(QueryParseError {
                kind: QueryParseErrorKind::UnknownKey,
                message: format!(
                    "unknown key '{}' (expected: type, contract, topic, topic0..topic3, ledger, tx)",
                    key
                ),
                position: key_start,
            });
        }

        // Skip the ':'
        pos += 1;

        if pos >= len
            || bytes[pos] == b' '
            || bytes[pos] == b'\t'
            || bytes[pos] == b')'
            || bytes[pos] == b'('
        {
            return Err(QueryParseError {
                kind: QueryParseErrorKind::MissingValue,
                message: format!("missing value for key '{}'", key),
                position: if pos >= len { len } else { pos },
            });
        }

        // Read value.
        let value_start = pos;
        let value;

        if bytes[pos] == b'"' {
            // Quoted value: read until unescaped closing ".
            pos += 1; // skip opening "
            let mut val = String::new();
            loop {
                if pos >= len {
                    return Err(QueryParseError {
                        kind: QueryParseErrorKind::UnbalancedQuotes,
                        message: "unbalanced quotes: missing closing '\"'".to_string(),
                        position: value_start,
                    });
                }
                if bytes[pos] == b'\\' && pos + 1 < len && bytes[pos + 1] == b'"' {
                    val.push('"');
                    pos += 2;
                } else if bytes[pos] == b'"' {
                    pos += 1; // skip closing "
                    break;
                } else {
                    val.push(bytes[pos] as char);
                    pos += 1;
                }
            }
            value = val;
        } else if bytes[pos] == b'{' {
            // JSON value with brace-depth tracking.
            let mut depth = 0;
            let json_start = pos;
            loop {
                if pos >= len {
                    return Err(QueryParseError {
                        kind: QueryParseErrorKind::UnbalancedBraces,
                        message: "unbalanced braces: missing closing '}'".to_string(),
                        position: json_start,
                    });
                }
                if bytes[pos] == b'"' {
                    // Skip over quoted strings inside JSON.
                    pos += 1;
                    while pos < len {
                        if bytes[pos] == b'\\' && pos + 1 < len {
                            pos += 2;
                        } else if bytes[pos] == b'"' {
                            pos += 1;
                            break;
                        } else {
                            pos += 1;
                        }
                    }
                    continue;
                }
                if bytes[pos] == b'{' {
                    depth += 1;
                } else if bytes[pos] == b'}' {
                    depth -= 1;
                    if depth == 0 {
                        pos += 1;
                        break;
                    }
                }
                pos += 1;
            }
            value = input[json_start..pos].to_string();
        } else {
            // Bare value: read until whitespace, ), or end.
            while pos < len && bytes[pos] != b' ' && bytes[pos] != b'\t' && bytes[pos] != b')' {
                pos += 1;
            }
            value = input[value_start..pos].to_string();
        }

        let text = input[start..pos].to_string();
        tokens.push(Token {
            kind: TokenKind::Qualifier,
            text,
            key: Some(key.to_string()),
            value: Some(value),
            position: start,
        });
    }

    Ok(tokens)
}

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum QueryExpr {
    Qualifier {
        key: String,
        value: String,
        position: usize,
    },
    And(Vec<QueryExpr>),
    Or(Vec<QueryExpr>),
}

// ---------------------------------------------------------------------------
// Parser (recursive descent)
// ---------------------------------------------------------------------------

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            pos: 0,
            depth: 0,
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn parse_or(&mut self) -> Result<QueryExpr, QueryParseError> {
        let mut left = self.parse_and()?;

        while let Some(tok) = self.peek() {
            if tok.kind != TokenKind::Or {
                break;
            }
            let or_pos = tok.position;
            self.pos += 1; // consume OR

            // OR must be followed by an expression.
            if self.pos >= self.tokens.len() {
                return Err(QueryParseError {
                    kind: QueryParseErrorKind::UnexpectedToken,
                    message: "unexpected end of query after OR".to_string(),
                    position: or_pos,
                });
            }

            let right = self.parse_and()?;
            left = match left {
                QueryExpr::Or(mut children) => {
                    children.push(right);
                    QueryExpr::Or(children)
                }
                _ => QueryExpr::Or(vec![left, right]),
            };
        }

        Ok(left)
    }

    fn parse_and(&mut self) -> Result<QueryExpr, QueryParseError> {
        let mut left = self.parse_atom()?;

        while let Some(tok) = self.peek() {
            // Stop at OR, RPAREN, or end.
            if tok.kind == TokenKind::Or || tok.kind == TokenKind::RParen {
                break;
            }
            let right = self.parse_atom()?;
            left = match left {
                QueryExpr::And(mut children) => {
                    children.push(right);
                    QueryExpr::And(children)
                }
                _ => QueryExpr::And(vec![left, right]),
            };
        }

        Ok(left)
    }

    fn parse_atom(&mut self) -> Result<QueryExpr, QueryParseError> {
        let tok = self.peek().ok_or_else(|| QueryParseError {
            kind: QueryParseErrorKind::UnexpectedToken,
            message: "unexpected end of query".to_string(),
            position: if self.tokens.is_empty() {
                0
            } else {
                self.tokens.last().unwrap().position
            },
        })?;

        match tok.kind {
            TokenKind::LParen => {
                let paren_pos = tok.position;
                self.pos += 1; // consume (

                self.depth += 1;
                if self.depth > MAX_NESTING_DEPTH {
                    return Err(QueryParseError {
                        kind: QueryParseErrorKind::NestingTooDeep,
                        message: format!(
                            "query exceeds maximum nesting depth of {}",
                            MAX_NESTING_DEPTH
                        ),
                        position: paren_pos,
                    });
                }

                // Check for empty parens.
                if let Some(next) = self.peek() {
                    if next.kind == TokenKind::RParen {
                        return Err(QueryParseError {
                            kind: QueryParseErrorKind::UnexpectedToken,
                            message: "unexpected ')' (empty parentheses)".to_string(),
                            position: next.position,
                        });
                    }
                }

                let expr = self.parse_or()?;

                self.depth -= 1;

                // Expect RPAREN.
                match self.peek() {
                    Some(t) if t.kind == TokenKind::RParen => {
                        self.pos += 1;
                        Ok(expr)
                    }
                    _ => Err(QueryParseError {
                        kind: QueryParseErrorKind::UnbalancedParens,
                        message: "unbalanced parentheses: missing closing ')'".to_string(),
                        position: paren_pos,
                    }),
                }
            }
            TokenKind::Qualifier => {
                let key = tok.key.clone().unwrap();
                let value = tok.value.clone().unwrap();
                let position = tok.position;
                self.pos += 1;
                Ok(QueryExpr::Qualifier {
                    key,
                    value,
                    position,
                })
            }
            TokenKind::Or => Err(QueryParseError {
                kind: QueryParseErrorKind::UnexpectedToken,
                message: "unexpected 'OR' (must be preceded by an expression)".to_string(),
                position: tok.position,
            }),
            TokenKind::RParen => Err(QueryParseError {
                kind: QueryParseErrorKind::UnbalancedParens,
                message: "unbalanced parentheses: unexpected ')'".to_string(),
                position: tok.position,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// DNF conversion
// ---------------------------------------------------------------------------

/// Convert an AST into Disjunctive Normal Form (OR of ANDs).
fn to_dnf(expr: QueryExpr) -> QueryExpr {
    match expr {
        QueryExpr::Qualifier { .. } => expr,
        QueryExpr::Or(children) => {
            let normalized: Vec<QueryExpr> = children.into_iter().map(to_dnf).collect();
            QueryExpr::Or(normalized)
        }
        QueryExpr::And(children) => {
            // First normalize all children.
            let normalized: Vec<QueryExpr> = children.into_iter().map(to_dnf).collect();
            // Distribute AND over OR.
            distribute_and(normalized)
        }
    }
}

/// Given a list of children that are AND'd, distribute AND over any OR children.
/// Returns a DNF expression.
fn distribute_and(children: Vec<QueryExpr>) -> QueryExpr {
    // Start with a single empty conjunction.
    let mut conjunctions: Vec<Vec<QueryExpr>> = vec![vec![]];

    for child in children {
        match child {
            QueryExpr::Or(or_children) => {
                // Each existing conjunction gets multiplied by each OR branch.
                let mut new_conjunctions = Vec::new();
                for existing in &conjunctions {
                    for branch in &or_children {
                        let mut extended = existing.clone();
                        match branch {
                            QueryExpr::And(and_children) => {
                                extended.extend(and_children.iter().cloned());
                            }
                            _ => {
                                extended.push(branch.clone());
                            }
                        }
                        new_conjunctions.push(extended);
                    }
                }
                conjunctions = new_conjunctions;
            }
            QueryExpr::And(and_children) => {
                for conj in &mut conjunctions {
                    conj.extend(and_children.iter().cloned());
                }
            }
            _ => {
                for conj in &mut conjunctions {
                    conj.push(child.clone());
                }
            }
        }
    }

    if conjunctions.len() == 1 {
        let conj = conjunctions.into_iter().next().unwrap();
        if conj.len() == 1 {
            conj.into_iter().next().unwrap()
        } else {
            QueryExpr::And(conj)
        }
    } else {
        QueryExpr::Or(
            conjunctions
                .into_iter()
                .map(|conj| {
                    if conj.len() == 1 {
                        conj.into_iter().next().unwrap()
                    } else {
                        QueryExpr::And(conj)
                    }
                })
                .collect(),
        )
    }
}

/// Flatten a DNF expression into a list of AND-groups.
/// Each AND-group is a Vec of Qualifier nodes.
fn flatten_or(expr: QueryExpr) -> Vec<Vec<(String, String, usize)>> {
    match expr {
        QueryExpr::Qualifier {
            key,
            value,
            position,
        } => vec![vec![(key, value, position)]],
        QueryExpr::And(children) => {
            let mut group = Vec::new();
            for child in children {
                match child {
                    QueryExpr::Qualifier {
                        key,
                        value,
                        position,
                    } => {
                        group.push((key, value, position));
                    }
                    _ => unreachable!("DNF should only contain qualifiers inside AND"),
                }
            }
            vec![group]
        }
        QueryExpr::Or(children) => {
            let mut groups = Vec::new();
            for child in children {
                groups.extend(flatten_or(child));
            }
            groups
        }
    }
}

// ---------------------------------------------------------------------------
// AND-group to EventFilter
// ---------------------------------------------------------------------------

fn and_group_to_filter(
    group: Vec<(String, String, usize)>,
) -> Result<EventFilter, QueryParseError> {
    let mut event_type: Option<(String, usize)> = None;
    let mut contract_id: Option<(String, usize)> = None;
    let mut ledger: Option<(u32, usize)> = None;
    let mut tx: Option<(String, usize)> = None;
    let mut topics: [Option<(String, usize)>; 4] = [None, None, None, None];
    let mut any_topics: Vec<String> = Vec::new();

    for (key, value, position) in group {
        match key.as_str() {
            "topic" => {
                // Validate the JSON value.
                serde_json::from_str::<serde_json::Value>(&value).map_err(|_| QueryParseError {
                    kind: QueryParseErrorKind::InvalidValue,
                    message: format!("invalid JSON value for 'topic': {}", value),
                    position,
                })?;

                // Multiple topic: values are allowed (AND'd). Duplicates collapsed.
                if !any_topics.contains(&value) {
                    any_topics.push(value);
                }
            }
            "type" => {
                // Validate event type value.
                value
                    .parse::<EventType>()
                    .map_err(|_| QueryParseError {
                        kind: QueryParseErrorKind::InvalidValue,
                        message: format!(
                            "invalid value '{}' for key 'type' (expected: contract, system, diagnostic)",
                            value
                        ),
                        position,
                    })?;

                if let Some((ref existing, _)) = event_type {
                    if *existing == value {
                        // Duplicate with same value: collapse.
                        continue;
                    }
                    return Err(QueryParseError {
                        kind: QueryParseErrorKind::ConflictingQualifiers,
                        message: format!(
                            "conflicting values for 'type': '{}' and '{}' (use OR to match multiple types)",
                            existing, value
                        ),
                        position,
                    });
                }
                event_type = Some((value, position));
            }
            "contract" => {
                if let Some((ref existing, _)) = contract_id {
                    if *existing == value {
                        continue;
                    }
                    return Err(QueryParseError {
                        kind: QueryParseErrorKind::ConflictingQualifiers,
                        message: format!(
                            "conflicting values for 'contract': '{}' and '{}' (use OR to match multiple contracts)",
                            existing, value
                        ),
                        position,
                    });
                }
                contract_id = Some((value, position));
            }
            "ledger" => {
                let parsed = value.parse::<u32>().map_err(|_| QueryParseError {
                    kind: QueryParseErrorKind::InvalidValue,
                    message: format!(
                        "invalid value '{}' for key 'ledger' (expected a positive integer)",
                        value
                    ),
                    position,
                })?;

                if let Some((existing, _)) = ledger {
                    if existing == parsed {
                        continue;
                    }
                    return Err(QueryParseError {
                        kind: QueryParseErrorKind::ConflictingQualifiers,
                        message: format!(
                            "conflicting values for 'ledger': '{}' and '{}' (use OR to match multiple ledgers)",
                            existing, parsed
                        ),
                        position,
                    });
                }
                ledger = Some((parsed, position));
            }
            "tx" => {
                if let Some((ref existing, _)) = tx {
                    if *existing == value {
                        continue;
                    }
                    return Err(QueryParseError {
                        kind: QueryParseErrorKind::ConflictingQualifiers,
                        message: format!(
                            "conflicting values for 'tx': '{}' and '{}' (use OR to match multiple transactions)",
                            existing, value
                        ),
                        position,
                    });
                }
                tx = Some((value, position));
            }
            topic_key @ ("topic0" | "topic1" | "topic2" | "topic3") => {
                let idx: usize = topic_key[5..].parse().unwrap();

                // Validate the JSON value.
                serde_json::from_str::<serde_json::Value>(&value).map_err(|_| QueryParseError {
                    kind: QueryParseErrorKind::InvalidValue,
                    message: format!("invalid JSON value for '{}': {}", topic_key, value),
                    position,
                })?;

                if let Some((ref existing, _)) = topics[idx] {
                    if *existing == value {
                        continue;
                    }
                    return Err(QueryParseError {
                        kind: QueryParseErrorKind::DuplicateTopicPosition,
                        message: format!(
                            "duplicate '{}' in one filter group (use OR to match multiple values)",
                            topic_key
                        ),
                        position,
                    });
                }
                topics[idx] = Some((value, position));
            }
            _ => unreachable!("key validated during tokenization"),
        }
    }

    // tx requires ledger
    if let Some((_, pos)) = &tx {
        if ledger.is_none() {
            return Err(QueryParseError {
                kind: QueryParseErrorKind::InvalidValue,
                message: "ledger is required when tx is provided".to_string(),
                position: *pos,
            });
        }
    }

    // Build topics vector.
    let topics_vec = {
        // Find the highest set topic index.
        let max_idx = topics.iter().rposition(|t| t.is_some());
        match max_idx {
            Some(max) => {
                let mut v = Vec::with_capacity(max + 1);
                for topic in topics.iter().take(max + 1) {
                    match topic {
                        Some((json_str, _)) => {
                            let val: serde_json::Value = serde_json::from_str(json_str).unwrap();
                            v.push(val);
                        }
                        None => {
                            v.push(serde_json::Value::Null);
                        }
                    }
                }
                Some(v)
            }
            None => None,
        }
    };

    let any_topics_vec = if any_topics.is_empty() {
        None
    } else {
        Some(
            any_topics
                .into_iter()
                .map(|s| serde_json::from_str(&s).unwrap())
                .collect(),
        )
    };

    Ok(EventFilter {
        event_type: event_type.map(|(v, _)| v),
        contract_id: contract_id.map(|(v, _)| v),
        topics: topics_vec,
        any_topics: any_topics_vec,
        ledger: ledger.map(|(v, _)| v),
        tx: tx.map(|(v, _)| v),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CA: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const CB: &str = "CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

    // --- 1.1 Single qualifier: type ---

    #[test]
    fn test_parse_single_type_contract() {
        let filters = parse_query("type:contract").unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].event_type.as_deref(), Some("contract"));
        assert_eq!(filters[0].contract_id, None);
        assert_eq!(filters[0].topics, None);
    }

    #[test]
    fn test_parse_single_type_system() {
        let filters = parse_query("type:system").unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].event_type.as_deref(), Some("system"));
    }

    #[test]
    fn test_parse_single_type_diagnostic() {
        let filters = parse_query("type:diagnostic").unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].event_type.as_deref(), Some("diagnostic"));
    }

    // --- 1.2 Single qualifier: contract ---

    #[test]
    fn test_parse_single_contract() {
        let filters = parse_query(&format!("contract:{}", CA)).unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].contract_id.as_deref(), Some(CA));
        assert_eq!(filters[0].event_type, None);
        assert_eq!(filters[0].topics, None);
    }

    // --- 1.3 Single qualifier: topic0 ---

    #[test]
    fn test_parse_single_topic0() {
        let filters = parse_query(r#"topic0:{"symbol":"transfer"}"#).unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].event_type, None);
        assert_eq!(filters[0].contract_id, None);
        let topics = filters[0].topics.as_ref().unwrap();
        assert_eq!(topics.len(), 1);
        assert_eq!(topics[0], json!({"symbol": "transfer"}));
    }

    // --- 1.4 Topic with nested JSON ---

    #[test]
    fn test_parse_topic_nested_json() {
        let filters = parse_query(r#"topic0:{"nested":{"a":"b"}}"#).unwrap();
        assert_eq!(filters.len(), 1);
        let topics = filters[0].topics.as_ref().unwrap();
        assert_eq!(topics[0], json!({"nested": {"a": "b"}}));
    }

    // --- 1.5 AND: type + contract ---

    #[test]
    fn test_parse_and_type_contract() {
        let filters = parse_query(&format!("type:contract contract:{}", CA)).unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].event_type.as_deref(), Some("contract"));
        assert_eq!(filters[0].contract_id.as_deref(), Some(CA));
        assert_eq!(filters[0].topics, None);
    }

    // --- 1.6 AND: type + topic0 + topic2 (gap at position 1) ---

    #[test]
    fn test_parse_and_type_topic0_topic2() {
        let filters =
            parse_query(r#"type:contract topic0:{"symbol":"transfer"} topic2:{"address":"GDEF"}"#)
                .unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].event_type.as_deref(), Some("contract"));
        let topics = filters[0].topics.as_ref().unwrap();
        assert_eq!(topics.len(), 3);
        assert_eq!(topics[0], json!({"symbol": "transfer"}));
        assert!(topics[1].is_null());
        assert_eq!(topics[2], json!({"address": "GDEF"}));
    }

    // --- 1.7 OR: two types ---

    #[test]
    fn test_parse_or_two_types() {
        let filters = parse_query("type:contract OR type:system").unwrap();
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].event_type.as_deref(), Some("contract"));
        assert_eq!(filters[1].event_type.as_deref(), Some("system"));
    }

    // --- 1.8 OR: three-way ---

    #[test]
    fn test_parse_or_three_way() {
        let filters = parse_query("type:contract OR type:system OR type:diagnostic").unwrap();
        assert_eq!(filters.len(), 3);
        assert_eq!(filters[0].event_type.as_deref(), Some("contract"));
        assert_eq!(filters[1].event_type.as_deref(), Some("system"));
        assert_eq!(filters[2].event_type.as_deref(), Some("diagnostic"));
    }

    // --- 1.9 Parenthesized group: OR inside AND ---

    #[test]
    fn test_parse_paren_or_and() {
        let filters =
            parse_query(&format!("(type:contract OR type:system) contract:{}", CA)).unwrap();
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].event_type.as_deref(), Some("contract"));
        assert_eq!(filters[0].contract_id.as_deref(), Some(CA));
        assert_eq!(filters[1].event_type.as_deref(), Some("system"));
        assert_eq!(filters[1].contract_id.as_deref(), Some(CA));
    }

    // --- 1.10 Parenthesized group: OR contracts with shared topic ---

    #[test]
    fn test_parse_paren_or_contracts_topic() {
        let filters = parse_query(&format!(
            r#"(contract:{} OR contract:{}) topic0:{{"symbol":"transfer"}}"#,
            CA, CB
        ))
        .unwrap();
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].contract_id.as_deref(), Some(CA));
        assert_eq!(
            filters[0].topics.as_ref().unwrap()[0],
            json!({"symbol": "transfer"})
        );
        assert_eq!(filters[1].contract_id.as_deref(), Some(CB));
        assert_eq!(
            filters[1].topics.as_ref().unwrap()[0],
            json!({"symbol": "transfer"})
        );
    }

    // --- 1.11 Nested parentheses ---

    #[test]
    fn test_parse_nested_parens() {
        let filters = parse_query("((type:contract))").unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].event_type.as_deref(), Some("contract"));
    }

    // --- 1.12 DNF cartesian product ---

    #[test]
    fn test_parse_dnf_cartesian_product() {
        let filters = parse_query(&format!(
            "(type:contract OR type:system) (contract:{} OR contract:{})",
            CA, CB
        ))
        .unwrap();
        assert_eq!(filters.len(), 4);
        // Verify all 4 combinations exist.
        let combos: Vec<(Option<&str>, Option<&str>)> = filters
            .iter()
            .map(|f| (f.event_type.as_deref(), f.contract_id.as_deref()))
            .collect();
        assert!(combos.contains(&(Some("contract"), Some(CA))));
        assert!(combos.contains(&(Some("contract"), Some(CB))));
        assert!(combos.contains(&(Some("system"), Some(CA))));
        assert!(combos.contains(&(Some("system"), Some(CB))));
    }

    // --- 1.13 Duplicate qualifier: same value collapsed ---

    #[test]
    fn test_parse_duplicate_same_value() {
        let filters = parse_query("type:contract type:contract").unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].event_type.as_deref(), Some("contract"));
    }

    // --- 1.14 Precedence: AND binds tighter than OR ---

    #[test]
    fn test_parse_precedence_and_over_or() {
        let filters = parse_query(
            r#"type:contract topic0:{"symbol":"transfer"} OR type:system topic0:{"symbol":"core_metrics"}"#,
        )
        .unwrap();
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].event_type.as_deref(), Some("contract"));
        assert_eq!(
            filters[0].topics.as_ref().unwrap()[0],
            json!({"symbol": "transfer"})
        );
        assert_eq!(filters[1].event_type.as_deref(), Some("system"));
        assert_eq!(
            filters[1].topics.as_ref().unwrap()[0],
            json!({"symbol": "core_metrics"})
        );
    }

    // --- 1.15 Extra whitespace ---

    #[test]
    fn test_parse_extra_whitespace() {
        let filters = parse_query("  type:contract  ").unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].event_type.as_deref(), Some("contract"));
    }

    // --- 1.16 Quoted value ---

    #[test]
    fn test_parse_quoted_value() {
        let filters = parse_query(r#"type:"contract""#).unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].event_type.as_deref(), Some("contract"));
    }

    // --- 1.20 Error cases ---

    #[test]
    fn test_parse_error_empty_query() {
        let err = parse_query("").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::EmptyQuery);
    }

    #[test]
    fn test_parse_error_whitespace_only() {
        let err = parse_query("   ").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::EmptyQuery);
    }

    #[test]
    fn test_parse_error_unknown_key() {
        let err = parse_query("foo:bar").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::UnknownKey);
        assert!(err.message.contains("foo"));
    }

    #[test]
    fn test_parse_error_missing_value() {
        let err = parse_query("type: ").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::MissingValue);
    }

    #[test]
    fn test_parse_error_missing_value_eoi() {
        // "type:" at end of input with nothing after colon should fail as
        // missing value. The tokenizer checks if pos >= len after consuming ':'.
        // In practice "type:" has a space-trimmed trailing, so test "type: "
        // which hits missing_value because next char is space.
        // Also test bare "type:" at absolute end by using it alone in a group.
        let err = parse_query("contract:CA type:").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::MissingValue);
    }

    #[test]
    fn test_parse_error_invalid_type_value() {
        let err = parse_query("type:bogus").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::InvalidValue);
    }

    #[test]
    fn test_parse_error_invalid_type_wrong_case() {
        let err = parse_query("type:CONTRACT").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::InvalidValue);
    }

    #[test]
    fn test_parse_error_unbalanced_open_paren() {
        let err = parse_query("(type:contract").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::UnbalancedParens);
    }

    #[test]
    fn test_parse_error_unbalanced_close_paren() {
        let err = parse_query("type:contract)").unwrap_err();
        // A stray ')' after a complete expression is flagged as an unexpected
        // token rather than unbalanced parens, since the parser has already
        // consumed a valid expression.
        assert!(
            err.kind == QueryParseErrorKind::UnexpectedToken
                || err.kind == QueryParseErrorKind::UnbalancedParens
        );
    }

    #[test]
    fn test_parse_error_empty_parens() {
        let err = parse_query("()").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::UnexpectedToken);
    }

    #[test]
    fn test_parse_error_leading_or() {
        let err = parse_query("OR type:contract").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::UnexpectedToken);
    }

    #[test]
    fn test_parse_error_trailing_or() {
        let err = parse_query("type:contract OR").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::UnexpectedToken);
    }

    #[test]
    fn test_parse_error_consecutive_or() {
        let err = parse_query("type:contract OR OR type:system").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::UnexpectedToken);
    }

    #[test]
    fn test_parse_error_conflicting_qualifiers() {
        let err = parse_query("type:contract type:system").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::ConflictingQualifiers);
    }

    #[test]
    fn test_parse_error_duplicate_topic_position() {
        let err = parse_query(r#"topic0:{"symbol":"a"} topic0:{"symbol":"b"}"#).unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::DuplicateTopicPosition);
    }

    #[test]
    fn test_parse_error_unbalanced_braces() {
        let err = parse_query(r#"topic0:{"symbol":"transfer""#).unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::UnbalancedBraces);
    }

    #[test]
    fn test_parse_error_unbalanced_quotes() {
        let err = parse_query(r#"type:"contract"#).unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::UnbalancedQuotes);
    }

    #[test]
    fn test_parse_error_too_many_filters() {
        // 3 * 2 * 4 = 24 > 20
        let q = format!(
            r#"(type:contract OR type:system OR type:diagnostic) (contract:{} OR contract:{}) (topic0:{{"symbol":"transfer"}} OR topic0:{{"symbol":"mint"}} OR topic0:{{"symbol":"diag"}} OR topic0:{{"symbol":"core_metrics"}})"#,
            CA, CB
        );
        let err = parse_query(&q).unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::TooManyFilters);
    }

    #[test]
    fn test_parse_error_conflicting_qualifiers_in_paren_group() {
        let err = parse_query("(type:contract type:system)").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::ConflictingQualifiers);
    }

    // --- topic (any position) tests ---

    #[test]
    fn test_parse_single_topic_any() {
        let filters = parse_query(r#"topic:{"symbol":"transfer"}"#).unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].topics, None);
        let any = filters[0].any_topics.as_ref().unwrap();
        assert_eq!(any.len(), 1);
        assert_eq!(any[0], json!({"symbol": "transfer"}));
    }

    #[test]
    fn test_parse_multiple_topic_any() {
        let filters =
            parse_query(r#"topic:{"symbol":"transfer"} topic:{"symbol":"mint"}"#).unwrap();
        assert_eq!(filters.len(), 1);
        let any = filters[0].any_topics.as_ref().unwrap();
        assert_eq!(any.len(), 2);
        assert_eq!(any[0], json!({"symbol": "transfer"}));
        assert_eq!(any[1], json!({"symbol": "mint"}));
    }

    #[test]
    fn test_parse_topic_any_duplicate_collapsed() {
        let filters =
            parse_query(r#"topic:{"symbol":"transfer"} topic:{"symbol":"transfer"}"#).unwrap();
        assert_eq!(filters.len(), 1);
        let any = filters[0].any_topics.as_ref().unwrap();
        assert_eq!(any.len(), 1);
    }

    #[test]
    fn test_parse_topic_any_with_type() {
        let filters = parse_query(r#"type:contract topic:{"symbol":"transfer"}"#).unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].event_type.as_deref(), Some("contract"));
        let any = filters[0].any_topics.as_ref().unwrap();
        assert_eq!(any[0], json!({"symbol": "transfer"}));
    }

    #[test]
    fn test_parse_topic_any_with_positional() {
        let filters =
            parse_query(r#"topic:{"symbol":"transfer"} topic0:{"symbol":"transfer"}"#).unwrap();
        assert_eq!(filters.len(), 1);
        let any = filters[0].any_topics.as_ref().unwrap();
        assert_eq!(any[0], json!({"symbol": "transfer"}));
        let positional = filters[0].topics.as_ref().unwrap();
        assert_eq!(positional[0], json!({"symbol": "transfer"}));
    }

    #[test]
    fn test_parse_topic_any_or_expansion() {
        let filters = parse_query(&format!(
            r#"(contract:{} OR contract:{}) topic:{{"symbol":"transfer"}}"#,
            CA, CB
        ))
        .unwrap();
        assert_eq!(filters.len(), 2);
        for f in &filters {
            let any = f.any_topics.as_ref().unwrap();
            assert_eq!(any[0], json!({"symbol": "transfer"}));
        }
        assert_eq!(filters[0].contract_id.as_deref(), Some(CA));
        assert_eq!(filters[1].contract_id.as_deref(), Some(CB));
    }

    #[test]
    fn test_parse_topic_any_invalid_json() {
        let err = parse_query("topic:notjson").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::InvalidValue);
    }

    // --- ledger and tx tests ---

    #[test]
    fn test_parse_single_ledger() {
        let filters = parse_query("ledger:58000000").unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].ledger, Some(58000000));
    }

    #[test]
    fn test_parse_ledger_with_type() {
        let filters = parse_query("ledger:100 type:contract").unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].ledger, Some(100));
        assert_eq!(filters[0].event_type.as_deref(), Some("contract"));
    }

    #[test]
    fn test_parse_ledger_invalid_value() {
        let err = parse_query("ledger:abc").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::InvalidValue);
        assert!(err.message.contains("invalid value"));
    }

    #[test]
    fn test_parse_ledger_conflicting() {
        let err = parse_query("ledger:100 ledger:200").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::ConflictingQualifiers);
    }

    #[test]
    fn test_parse_ledger_duplicate_same_value() {
        let filters = parse_query("ledger:100 ledger:100").unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].ledger, Some(100));
    }

    #[test]
    fn test_parse_tx_with_ledger() {
        let tx = "a".repeat(64);
        let filters = parse_query(&format!("ledger:100 tx:{}", tx)).unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].ledger, Some(100));
        assert_eq!(filters[0].tx.as_deref(), Some(tx.as_str()));
    }

    #[test]
    fn test_parse_tx_without_ledger_error() {
        let tx = "a".repeat(64);
        let err = parse_query(&format!("tx:{}", tx)).unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::InvalidValue);
        assert!(err.message.contains("ledger is required"));
    }

    #[test]
    fn test_parse_tx_conflicting() {
        let err = parse_query("ledger:100 tx:abc tx:def").unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::ConflictingQualifiers);
    }

    #[test]
    fn test_parse_tx_duplicate_same_value() {
        let filters = parse_query("ledger:100 tx:abc tx:abc").unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].tx.as_deref(), Some("abc"));
    }

    // --- Complexity limit tests ---

    #[test]
    fn test_parse_error_query_too_long() {
        // Build a query that exceeds 1024 bytes.
        // Use repeated OR'd contract qualifiers to build length.
        let mut q = format!("contract:{}", CA);
        while q.len() <= MAX_QUERY_LENGTH {
            q.push_str(&format!(" OR contract:{}", CA));
        }
        assert!(q.len() > MAX_QUERY_LENGTH);
        let err = parse_query(&q).unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::QueryTooLong);
    }

    #[test]
    fn test_parse_query_at_max_length() {
        // Build a query that is exactly MAX_QUERY_LENGTH bytes.
        let base = "type:contract";
        let padding_len = MAX_QUERY_LENGTH - base.len();
        let q = format!("{}{}", base, " ".repeat(padding_len));
        assert_eq!(q.len(), MAX_QUERY_LENGTH);
        let result = parse_query(&q);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_error_too_many_terms() {
        // Build a query with 21 terms (over the limit of 20).
        let terms: Vec<String> = (0..21).map(|_| "type:contract".to_string()).collect();
        // All same value so they'll collapse without conflicting, but
        // the term count check happens before filter construction.
        let q = terms.join(" ");
        let err = parse_query(&q).unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::TooManyTerms);
    }

    #[test]
    fn test_parse_query_at_max_terms() {
        // Build a query with exactly 20 terms — should succeed.
        // Use OR to avoid conflicting qualifier errors within one group.
        let terms: Vec<String> = (0..20).map(|_| "type:contract".to_string()).collect();
        let q = terms.join(" OR ");
        let result = parse_query(&q);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_error_nesting_too_deep() {
        // 5 levels of nesting — exceeds the limit of 4.
        let q = "(((((type:contract)))))";
        let err = parse_query(q).unwrap_err();
        assert_eq!(err.kind, QueryParseErrorKind::NestingTooDeep);
    }

    #[test]
    fn test_parse_query_at_max_nesting_depth() {
        // 4 levels of nesting — exactly at the limit.
        let q = "((((type:contract))))";
        let result = parse_query(q);
        assert!(result.is_ok());
    }
}
