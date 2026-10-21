//! `/api/status` handler.

use crate::state::AppState;
use axum::{Json, Router, extract::State, routing::get};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct Status {
    name: &'static str,
    version: &'static str,
    engine: &'static str,
    routes: usize,
    requests: u64,
    failures: u64,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/status", get(status))
}

async fn status(State(state): State<Arc<AppState>>) -> Json<Status> {
    Json(Status {
        name: "Passage",
        version: env!("CARGO_PKG_VERSION"),
        engine: "Pingora",
        routes: state.snapshot.load().active_routes(),
        requests: state.metrics.requests(),
        failures: state.metrics.failures(),
    })
}
