use crate::{
    certificates::Certificates,
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
    pub certificates: Arc<Certificates>,
    store: Arc<Store>,
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
    pub fn new(
        store: Arc<Store>,
        snapshot: Snapshot,
        admin_token: AdminToken,
        certificates: Arc<Certificates>,
    ) -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(snapshot),
            metrics: Metrics::default(),
            admin_token,
            certificates,
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
        let https_host = match &change {
            Change::Create(route) | Change::Update(route)
                if route.input.enabled && route.input.https.enabled =>
            {
                Some(route.input.host.as_str())
            }
            _ => None,
        };
        if let Some(host) = https_host {
            self.certificates
                .check_for(host)
                .map_err(UpdateError::Invalid)?;
        }
        self.store.apply(&change).map_err(UpdateError::Storage)?;
        self.snapshot.store(Arc::new(snapshot));
        tracing::info!(
            action = change.action(),
            route_id = change.route_id(),
            "configuration updated"
        );
        // Only once the route is saved, so a rejected change never leaves an order behind.
        if https_host.is_some()
            && let Err(error) = self.certificates.request_missing().await
        {
            tracing::error!(
                error = format!("{error:#}"),
                "cannot record the certificate order; the manager retries"
            );
        }
        Ok(())
    }

    /// Deletes a certificate unless a route still needs it; serialized with route changes.
    pub async fn delete_certificate(&self, id: &str) -> Result<(), UpdateError> {
        let _guard = self.writes.lock().await;
        let routes = self.store.load().map_err(UpdateError::Storage)?;
        self.certificates.delete(id, &routes).await
    }
}
