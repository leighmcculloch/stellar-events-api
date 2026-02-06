use serde::Serialize;

use crate::db::EventRow;

/// Stripe-style list response envelope.
#[derive(Debug, Serialize)]
pub struct ListResponse<T: Serialize> {
    pub url: String,
    pub has_more: bool,
    pub object: &'static str,
    pub data: Vec<T>,
}

/// A Stellar contract event, formatted for the API response.
#[derive(Debug, Serialize)]
pub struct Event {
    pub object: &'static str,
    pub id: String,
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
        let topics: serde_json::Value =
            serde_json::from_str(&row.topics_json).unwrap_or(serde_json::Value::Null);
        let data: serde_json::Value =
            serde_json::from_str(&row.data_json).unwrap_or(serde_json::Value::Null);

        // Convert unix timestamp to ISO 8601
        let closed_at = chrono::DateTime::from_timestamp(row.ledger_closed_at, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();

        Event {
            id: row.id,
            object: "event",
            event_type: row.event_type,
            ledger_sequence: row.ledger_sequence,
            ledger_closed_at: closed_at,
            contract_id: row.contract_id,
            tx_hash: row.tx_hash,
            topics,
            data,
        }
    }
}

/// Stripe-style error response.
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
