//! Shared example-only wire protocol for the DCA and exchange contracts.

use cosmwasm_std::{Coin, Uint128};
pub use durable_runtime::Correlation;
use serde::{Deserialize, Serialize};

/// Maximum number of finite, zero-based slices in an order.
pub const MAX_SLICES: u32 = 128;
/// Maximum JSON-encoded successful result size, in bytes.
pub const MAX_RESULT_BYTES: u32 = 2048;

/// A slice request. The actual sender is the callback target.
///
/// `correlation.workflow_id` identifies the host task, independently of the
/// contract-local `order_id`. `slice_index` is zero-based and independent of
/// `correlation.wait_sequence`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapRequest {
    pub order_id: u64,
    pub slice_index: u32,
    pub correlation: Correlation,
    pub input: Coin,
    pub output_denom: String,
    pub minimum_output: Uint128,
    pub beneficiary: String,
}

/// The result delivered to the requester after the beneficiary transfer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapReceipt {
    pub order_id: u64,
    pub slice_index: u32,
    pub correlation: Correlation,
    pub input: Coin,
    pub output: Coin,
    pub beneficiary: String,
}

/// Immutable fixed-price exchange terms, also returned by the config query.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeConfig {
    pub input_denom: String,
    pub output_denom: String,
    pub numerator: Uint128,
    pub denominator: Uint128,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeExecuteMsg {
    Swap(SwapRequest),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DcaCallbackMsg {
    SwapResult { receipt: SwapReceipt },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeQueryMsg {
    /// Returns `ExchangeConfig`.
    Config {},
    /// Returns `Option<SwapReceipt>`, scoped by the actual requesting DCA.
    Receipt {
        dca: String,
        order_id: u64,
        slice_index: u32,
    },
}
