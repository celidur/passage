//! `/api/upstreams`: live backend health and load per route.

use crate::{state::AppState, upstream::BackendStatus};
use axum::{Json, Router, extract::State, routing::get};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct RouteUpstreams {
    route_id: String,
    /// Requests currently admitted for the route.
    active: u32,
    backends: Vec<BackendStatus>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/upstreams", get(list))
}

async fn list(State(state): State<Arc<AppState>>) -> Json<Vec<RouteUpstreams>> {
    let snapshot = state.snapshot.load();
    let mut routes: Vec<_> = snapshot
        .pools()
        .iter()
        .map(|(id, pool)| RouteUpstreams {
            route_id: id.clone(),
            active: pool.active(),
            backends: pool.status(),
        })
        .collect();
    routes.sort_by(|a, b| a.route_id.cmp(&b.route_id));
    Json(routes)
}
