//! Background service that drives DNS refresh and active health checks for the live routes.

use crate::state::AppState;
use async_trait::async_trait;
use pingora::{server::ShutdownWatch, services::background::BackgroundService};
use std::{sync::Arc, time::Duration};
use tokio::time::{MissedTickBehavior, interval};

pub struct Maintenance {
    state: Arc<AppState>,
    dns_refresh: Duration,
}

impl Maintenance {
    pub fn new(state: Arc<AppState>, dns_refresh: Duration) -> Self {
        Self { state, dns_refresh }
    }
}

#[async_trait]
impl BackgroundService for Maintenance {
    async fn start(&self, mut shutdown: ShutdownWatch) {
        let mut ticker = interval(Duration::from_secs(1));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = shutdown.changed() => return,
                _ = ticker.tick() => {}
            }
            // Pools of replaced snapshots are simply no longer ticked.
            for pool in self.state.snapshot.load().pools().values() {
                pool.tick(self.dns_refresh);
            }
        }
    }
}
