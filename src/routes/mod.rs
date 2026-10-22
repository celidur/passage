//! Route definitions, validation and the compiled routing table.

mod model;
mod path;
mod snapshot;

pub use model::{Route, RouteInput};
pub use path::canonical_path;
pub use snapshot::{CompiledRoute, Snapshot};
