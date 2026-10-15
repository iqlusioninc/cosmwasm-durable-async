use crate::state::OrderRecord;
use cosmwasm_std::{Coin, Decimal, Uint128};
use dca_example_protocol::{Correlation, SwapReceipt};
use native_async::{AsyncExecuteMsg, AsyncQueryMsg, TaskResponse};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstantiateMsg {
    pub exchange: String,
    pub fee_denom: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateOrderRequest {
    pub input_per_slice: Coin,
    pub slice_count: u32,
    pub minimum_output: Uint128,
    pub interval_blocks: u64,
    pub beneficiary: String,
    pub gas: Coin,
    pub gas_per_poll: u64,
    pub max_gas_price: Decimal,
    pub expires_at: u64,
    pub allocated_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecuteMsg {
    CreateOrder(CreateOrderRequest),
    Async(AsyncExecuteMsg),
    UpdateBudget {
        order_id: u64,
        gas_per_poll: u64,
        max_gas_price: Decimal,
    },
    ReclaimPrincipal {
        order_id: u64,
    },
    DispatchSlice {
        order_id: u64,
        slice_index: u32,
        correlation: Correlation,
    },
    SwapResult {
        receipt: SwapReceipt,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryMsg {
    Order { order_id: u64 },
    Fill { order_id: u64, slice_index: u32 },
    Task { order_id: u64 },
    Async(AsyncQueryMsg),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateOrderResponse {
    pub order_id: u64,
    pub task_id: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegisterReplyPayload {
    pub order_id: u64,
}

/// Matches the generated workflow's single argument input envelope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkflowInput {
    pub order_id: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderView {
    pub order: OrderRecord,
    pub host: HostTaskView,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // Keep the approved public Present(TaskResponse) API.
pub enum HostTaskView {
    Present(TaskResponse),
    Unavailable { error: String, lease_expired: bool },
}
