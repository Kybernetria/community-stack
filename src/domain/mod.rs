//! Stable business vocabulary shared by use cases and adapters.
//!
//! This module contains data only. It must not import infrastructure engines,
//! runtimes, transports, or any adapter implementation.

pub mod documents;
pub mod facts;
pub mod planning;
pub mod profiles;
pub mod replication;
pub mod security;
pub mod toolkit;

pub use documents::*;
pub use facts::*;
pub use planning::*;
pub use profiles::*;
pub use replication::*;
pub use security::*;
pub use toolkit::*;

pub const MAX_UPDATE_BYTES: usize = 262_144;
