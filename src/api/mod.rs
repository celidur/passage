//! The loopback-only management API and embedded dashboard.

mod assets;
mod error;
mod json;
mod middleware;
mod routes;
mod status;
mod upstreams;

use crate::state::AppState;
use anyhow::Result;
use async_trait::async_trait;
use axum::{Json, Router, extract::DefaultBodyLimit, routing::get};
use pingora::{server::ShutdownWatch, services::background::BackgroundService};
use serde_json::json;
use std::{
    net::{SocketAddr, TcpListener},
    sync::{Arc, Mutex},
};

const BODY_LIMIT: usize = 64 * 1024;

pub fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .merge(status::router())
        .merge(routes::router())
        .merge(upstreams::router())
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::require_admin,
        ));
    Router::new()
        .route(
            "/healthz",
            get(|| async { Json(json!({ "status": "ok" })) }),
        )
        .nest("/api", api)
        .fallback(get(assets::serve))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(axum::middleware::from_fn(middleware::security_headers))
        .with_state(state)
}

/// Binds before Pingora starts so that address errors are reported at startup.
pub fn bind(address: SocketAddr) -> Result<TcpListener> {
    let listener = TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// Runs the management API as a Pingora background service, sharing its shutdown signal.
pub struct Management {
    state: Arc<AppState>,
    // Taken once by `start`, which only receives `&self`.
    listener: Mutex<Option<TcpListener>>,
}

impl Management {
    pub fn new(state: Arc<AppState>, listener: TcpListener) -> Self {
        Self {
            state,
            listener: Mutex::new(Some(listener)),
        }
    }
}

#[async_trait]
impl BackgroundService for Management {
    async fn start(&self, mut shutdown: ShutdownWatch) {
        let listener = self
            .listener
            .lock()
            .unwrap()
            .take()
            .expect("management starts once");
        let listener =
            tokio::net::TcpListener::from_std(listener).expect("nonblocking management listener");
        let shutdown = async move {
            let _ = shutdown.changed().await;
        };
        if let Err(error) = axum::serve(listener, router(self.state.clone()))
            .with_graceful_shutdown(shutdown)
            .await
        {
            tracing::error!(%error, "management server stopped");
        }
    }
}
