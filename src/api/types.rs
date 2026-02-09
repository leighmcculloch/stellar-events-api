use serde::Serialize;

use crate::db::EventRow;

/// Paginated list response envelope.
#[derive(Debug, Serialize)]
pub struct ListResponse<T: Serialize> {
    pub url: String,
    pub has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    pub object: &'static str,
    pub data: Vec<T>,
}

/// A Stellar contract event, formatted for the API response.
#[derive(Debug, Serialize)]
pub struct Event {
    pub object: &'static str,
    pub id: String,
    pub url: String,
    pub ledger_sequence: u32,
    pub ledger_closed_at: String,
    pub tx_hash: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub contract_id: Option<String>,
    pub topics: serde_json::Value,
    pub data: serde_json::Value,
}

impl From<EventRow> for Event {
    fn from(row: EventRow) -> Self {
        let url = format!("/events/{}", row.id);
        Event {
            id: row.id,
            url,
            object: "event",
            event_type: row.event_type.to_string(),
            ledger_sequence: row.ledger_sequence,
            ledger_closed_at: row.ledger_closed_at,
            contract_id: row.contract_id,
            tx_hash: row.tx_hash,
            topics: row.topics,
            data: row.data,
        }
    }
}

/// Structured error response.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: ErrorBody,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
}

/// Server status response.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub status: String,
    pub earliest_ledger: Option<u32>,
    pub latest_ledger: Option<u32>,
    pub network_passphrase: String,
    pub version: String,
}
