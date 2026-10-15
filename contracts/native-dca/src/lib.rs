//! Native DCA example with principal custody separate from host gas escrow.
pub mod contract;
pub mod error;
pub mod msg;
pub mod state;
pub use contract::{execute, instantiate, query, reply};
