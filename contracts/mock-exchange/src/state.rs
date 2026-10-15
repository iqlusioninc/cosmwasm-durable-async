use cosmwasm_std::{from_json, to_json_vec, Addr, StdError, StdResult, Storage};
use dca_example_protocol::{ExchangeConfig, SwapReceipt};

const CONFIG: &[u8] = b"mock-exchange/config";

pub fn save_config(storage: &mut dyn Storage, config: &ExchangeConfig) -> StdResult<()> {
    storage.set(CONFIG, &to_json_vec(config)?);
    Ok(())
}

pub fn load_config(storage: &dyn Storage) -> StdResult<ExchangeConfig> {
    from_json(
        storage
            .get(CONFIG)
            .ok_or_else(|| StdError::not_found("config"))?,
    )
}

fn receipt_key(requester: &Addr, order_id: u64, slice_index: u32) -> Vec<u8> {
    // Length-prefix the validated requester so concatenated identifiers cannot alias.
    let mut key = b"mock-exchange/receipt/".to_vec();
    key.extend_from_slice(&(requester.as_bytes().len() as u32).to_be_bytes());
    key.extend_from_slice(requester.as_bytes());
    key.extend_from_slice(&order_id.to_be_bytes());
    key.extend_from_slice(&slice_index.to_be_bytes());
    key
}

pub fn load_receipt(
    storage: &dyn Storage,
    requester: &Addr,
    order_id: u64,
    slice_index: u32,
) -> StdResult<Option<SwapReceipt>> {
    storage
        .get(&receipt_key(requester, order_id, slice_index))
        .map(from_json)
        .transpose()
}

pub fn save_receipt(
    storage: &mut dyn Storage,
    requester: &Addr,
    receipt: &SwapReceipt,
    encoded: &[u8],
) {
    storage.set(
        &receipt_key(requester, receipt.order_id, receipt.slice_index),
        encoded,
    );
}
