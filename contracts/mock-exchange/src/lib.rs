//! Immutable fixed-price native-coin exchange for local DCA examples.
pub mod contract;
pub mod error;
pub mod state;
pub use contract::{execute, instantiate, query};
pub use dca_example_protocol::ExchangeConfig as InstantiateMsg;
