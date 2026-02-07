use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use stellar_events_api::api;
use stellar_events_api::db::EventStore;
use stellar_events_api::ledger::fetch::fetch_config;
use stellar_events_api::sync::run_sync;
use stellar_events_api::AppState;

const DEFAULT_META_URL: &str =
    "https://aws-public-blockchain.s3.us-east-2.amazonaws.com/v1.1/stellar/ledgers/pubnet";

#[derive(Parser)]
#[command(
    name = "stellar-events-api",
    about = "HTTP API server for Stellar network contract events",
    version
)]
struct Cli {
    /// Port to listen on
    #[arg(long, default_value = "3000", env = "PORT")]
    port: u16,

    /// Bind address
    #[arg(long, default_value = "0.0.0.0", env = "BIND_ADDRESS")]
    bind: String,

    /// Base URL for the ledger metadata store
    #[arg(long, default_value = DEFAULT_META_URL, env = "META_URL")]
    meta_url: String,

    /// Ledger sequence to start syncing from (if not resuming)
    #[arg(long, env = "START_LEDGER")]
    start_ledger: Option<u32>,

    /// Number of ledgers to fetch concurrently during sync
    #[arg(long, default_value = "10", env = "PARALLEL_FETCHES")]
    parallel_fetches: u32,

    /// How long to keep cached ledger data, in days
    #[arg(long, default_value = "1", env = "CACHE_TTL_DAYS")]
    cache_ttl_days: u32,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // Fetch store configuration
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let store_config = match fetch_config(&client, &cli.meta_url).await {
        Ok(config) => config,
        Err(e) => {
            tracing::warn!(error = %e, "failed to fetch store config, using defaults");
            stellar_events_api::ledger::path::StoreConfig::default()
        }
    };

    let cache_ttl_seconds = cli.cache_ttl_days as i64 * 24 * 60 * 60;
    let store = EventStore::new(cache_ttl_seconds);

    tracing::info!("initialised in-memory event store");

    let state = Arc::new(AppState {
        store,
        config: store_config.clone(),
        meta_url: cli.meta_url.clone(),
        client: client.clone(),
    });

    // Start background sync
    let sync_state = Arc::clone(&state);
    let sync_url = cli.meta_url.clone();
    let sync_config = store_config.clone();
    let start_ledger = cli.start_ledger;
    let parallel_fetches = cli.parallel_fetches;
    tokio::spawn(async move {
        run_sync(
            client,
            sync_url,
            sync_config,
            sync_state,
            start_ledger,
            parallel_fetches,
        )
        .await;
    });

    // Build and start HTTP server
    let app = api::router(state);
    let addr: SocketAddr = format!("{}:{}", cli.bind, cli.port).parse()?;
    tracing::info!(address = %addr, "starting server");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
