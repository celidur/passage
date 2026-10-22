//! Upstream pools: load balancing, health tracking and DNS refresh.

mod check;
mod maintenance;
mod pool;
mod resolver;

pub use maintenance::Maintenance;
pub use pool::{BackendStatus, Lease, Pool, PoolSpec, Slot};
pub use resolver::{Resolver, SystemResolver};
