use crate::{
    metrics::Metrics,
    routes::{Route, Snapshot},
    store::{Change, Store},
    token::AdminToken,
};
use arc_swap::ArcSwap;
use std::sync::Arc;

/// State shared by the proxy and the management API.
pub struct AppState {
    /// Live routing table read by the proxy on every request.
    pub snapshot: ArcSwap<Snapshot>,
    pub metrics: Metrics,
    pub admin_token: AdminToken,
    store: Store,
    writes: tokio::sync::Mutex<()>,
}

/// Why a configuration change was not applied.
#[derive(Debug)]
pub enum UpdateError {
    NotFound,
    Invalid(anyhow::Error),
    Storage(anyhow::Error),
}

impl AppState {
    pub fn new(store: Store, snapshot: Snapshot, admin_token: AdminToken) -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(snapshot),
            metrics: Metrics::default(),
            admin_token,
            store,
            writes: tokio::sync::Mutex::new(()),
        }
    }

    pub fn routes(&self) -> anyhow::Result<Vec<Route>> {
        self.store.load()
    }

    /// Validates the resulting route set, persists the change, then publishes the new snapshot.
    /// A rejected change leaves both stored and live configuration untouched.
    pub async fn apply(&self, change: Change) -> Result<(), UpdateError> {
        let _guard = self.writes.lock().await;
        let mut routes = self.store.load().map_err(UpdateError::Storage)?;
        match &change {
            Change::Create(route) => routes.push(route.clone()),
            Change::Update(route) => {
                let existing = routes.iter_mut().find(|r| r.id == route.id);
                *existing.ok_or(UpdateError::NotFound)? = route.clone();
            }
            Change::Delete(id) => {
                let index = routes.iter().position(|r| &r.id == id);
                routes.remove(index.ok_or(UpdateError::NotFound)?);
            }
        }
        let snapshot = self
            .snapshot
            .load_full()
            .recompile(routes)
            .await
            .map_err(UpdateError::Invalid)?;
        self.store.apply(&change).map_err(UpdateError::Storage)?;
        self.snapshot.store(Arc::new(snapshot));
        tracing::info!(
            action = change.action(),
            route_id = change.route_id(),
            "configuration updated"
        );
        Ok(())
    }
}
