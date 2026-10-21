use std::sync::atomic::{AtomicU64, Ordering};

/// Process-wide proxy counters, reset on restart.
#[derive(Debug, Default)]
pub struct Metrics {
    requests: AtomicU64,
    failures: AtomicU64,
}

impl Metrics {
    pub fn record_request(&self) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn requests(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }

    pub fn failures(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }
}
