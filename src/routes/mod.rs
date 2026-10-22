//! Route definitions, validation and the compiled routing table.

mod model;
mod path;
pub mod policy;
mod snapshot;

pub use model::{Https, Route, RouteInput};
pub(crate) use model::{endpoint, normalize_host};
pub use path::canonical_path;
pub use policy::{Balancing, Policy, UpstreamProtocol};
pub use snapshot::{CompiledRoute, Snapshot};
