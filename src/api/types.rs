use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::db::EventRow;

/// JSON response wrapper that pretty-prints the output.
pub struct PrettyJson<T>(pub T);

impl<T: Serialize> IntoResponse for PrettyJson<T> {
    fn into_response(self) -> Response {
        match serde_json::to_vec_pretty(&self.0) {
            Ok(bytes) => ([(header::CONTENT_TYPE, "application/json")], bytes).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CONTENT_TYPE, "text/plain")],
                e.to_string(),
            )
                .into_response(),
        }
    }
}

/// Paginated list response envelope.
#[derive(Debug, Serialize)]
pub struct ListResponse<T: Serialize> {
    pub url: String,
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
    #[serde(rename = "ledger")]
    pub ledger_sequence: u32,
    #[serde(rename = "at")]
    pub ledger_closed_at: String,
    #[serde(rename = "tx")]
    pub tx_hash: String,
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(rename = "contract")]
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
    pub latest_ledger: Option<u32>,
    pub cached_ledgers: usize,
    pub network_passphrase: String,
    pub build: BuildInfo,
}

/// Build metadata embedded at compile time.
#[derive(Debug, Serialize)]
pub struct BuildInfo {
    pub repo: &'static str,
    pub branch: &'static str,
    pub commit: &'static str,
}
