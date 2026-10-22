//! Route definitions, validation and the compiled routing table.

mod model;
mod path;
pub mod policy;
mod snapshot;

pub(crate) use model::endpoint;
pub use model::{Route, RouteInput};
pub use path::canonical_path;
pub use policy::{Balancing, Policy, UpstreamProtocol};
pub use snapshot::{CompiledRoute, Snapshot};
