use cosmwasm_std::{from_json, to_json_vec, Addr, Coin, StdError, StdResult, Storage, Uint128};
use dca_example_protocol::{SwapReceipt, SwapRequest};
use serde::{Deserialize, Serialize};

pub const CONFIG_KEY: &[u8] = b"dca/v1/config";
pub const NEXT_ORDER_ID_KEY: &[u8] = b"dca/v1/next_order_id";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub exchange: Addr,
    pub fee_denom: String,
    pub input_denom: String,
    pub output_denom: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingDispatch {
    pub request: SwapRequest,
    pub dispatch_height: u64,
    pub deadline: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderRecord {
    pub order_id: u64,
    pub task_id: Option<u64>,
    pub owner: Addr,
    pub beneficiary: Addr,
    pub exchange: Addr,
    pub input_per_slice: Coin,
    pub output_denom: String,
    pub slice_count: u32,
    pub filled_count: u32,
    pub minimum_output: Uint128,
    pub initial_principal: Uint128,
    pub remaining: Uint128,
    pub reclaimed: Uint128,
    pub spent: Uint128,
    /// Output already delivered to the beneficiary, never contract custody.
    pub received: Uint128,
    pub interval_blocks: u64,
    pub registration_height: u64,
    pub expires_at: u64,
    pub pending: Option<PendingDispatch>,
    pub last_fill_height: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FillRecord {
    pub receipt: SwapReceipt,
    pub height: u64,
}

pub fn load_config(storage: &dyn Storage) -> StdResult<Config> {
    from_json(
        storage
            .get(CONFIG_KEY)
            .ok_or_else(|| StdError::not_found("DCA config"))?,
    )
}
pub fn load_order(storage: &dyn Storage, order_id: u64) -> StdResult<OrderRecord> {
    from_json(
        storage
            .get(&order_key(order_id))
            .ok_or_else(|| StdError::not_found("DCA order"))?,
    )
}
pub fn save_order(storage: &mut dyn Storage, order: &OrderRecord) -> StdResult<()> {
    storage.set(&order_key(order.order_id), &to_json_vec(order)?);
    Ok(())
}
pub fn load_fill(
    storage: &dyn Storage,
    order_id: u64,
    slice_index: u32,
) -> StdResult<Option<FillRecord>> {
    storage
        .get(&fill_key(order_id, slice_index))
        .map(from_json)
        .transpose()
}
pub fn save_fill(storage: &mut dyn Storage, fill: &FillRecord) -> StdResult<()> {
    storage.set(
        &fill_key(fill.receipt.order_id, fill.receipt.slice_index),
        &to_json_vec(fill)?,
    );
    Ok(())
}
fn order_key(order_id: u64) -> Vec<u8> {
    format!("dca/v1/order/{order_id:020}").into_bytes()
}
fn fill_key(order_id: u64, slice_index: u32) -> Vec<u8> {
    format!("dca/v1/fill/{order_id:020}/{slice_index:010}").into_bytes()
}
