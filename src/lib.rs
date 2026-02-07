pub mod api;
pub mod db;
pub mod ledger;
pub mod sync;

use std::sync::Mutex;

use db::EventStore;
use ledger::path::StoreConfig;

/// Shared application state.
///
/// Uses separate read and write database connections. SQLite in WAL mode allows
/// concurrent readers while one writer is active. The writer is behind a Mutex
/// (used by sync and backfill). The reader has its own connection and is also
/// behind a Mutex (since rusqlite::Connection is not Sync), but read locks are
/// never blocked by writes in WAL mode.
pub struct AppState {
    pub writer: Mutex<EventStore>,
    pub reader: Mutex<EventStore>,
    pub config: StoreConfig,
    pub meta_url: String,
    pub client: reqwest::Client,
    pub cache_ttl_seconds: i64,
}

/// Application-wide error type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("XDR parsing error: {0}")]
    Xdr(#[from] stellar_xdr::curr::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("ledger {0} not found")]
    LedgerNotFound(u32),

    #[error("config not found at {0}")]
    ConfigNotFound(String),

    #[error("internal error: {0}")]
    Internal(String),
}
