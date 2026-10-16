pub mod admission;
pub mod claims;
pub mod commitment;
pub mod contract;
pub mod error;
pub mod math;
pub mod msg;
pub mod queries;
pub mod settlement;
pub mod state;
pub mod workflow;
pub use contract::{execute, instantiate, query, reply, sudo};
pub use queries::{AuctionView, BiddingPhase, HostTaskView};

pub const MAX_CHECKPOINT_BYTES: usize = 4096;
pub const MAX_RESULT_BYTES: u32 = 64;
// Host metadata plus bounded timer/result and checkpoint envelope overhead.
// The auction and bid book are ordinary application storage, outside this lease.
pub const MIN_ALLOCATED_BYTES: u64 =
    native_async::METADATA_ALLOWANCE + native_async::inbox_reserve(MAX_RESULT_BYTES) + 16384;
