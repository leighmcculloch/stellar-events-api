pub mod error;
pub mod routes;
pub mod types;

use std::sync::Arc;

use axum::Router;
use metrics_exporter_prometheus::PrometheusHandle;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::AppState;

/// Build the API router.
pub fn router(state: Arc<AppState>, metrics_handle: Option<PrometheusHandle>) -> Router {
    let mut app = Router::new()
        .route("/", axum::routing::get(routes::home))
        .route(
            "/events",
            axum::routing::get(routes::list_events_get).post(routes::list_events_post),
        )
        .route("/health", axum::routing::get(routes::health));

    if let Some(handle) = metrics_handle {
        app = app.route(
            "/metrics",
            axum::routing::get(move || std::future::ready(handle.render())),
        );
    }

    app.layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
