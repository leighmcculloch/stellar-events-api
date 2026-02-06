pub mod error;
pub mod routes;
pub mod types;

use std::sync::Arc;

use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::AppState;

/// Build the API router.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", axum::routing::get(routes::home))
        .route(
            "/v1/events",
            axum::routing::get(routes::list_events_get).post(routes::list_events_post),
        )
        .route("/v1/health", axum::routing::get(routes::health))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
