//! Deliberately faulty acceptance counterparty. Never used by the tutorial.
//! Self-contained to avoid linking another contract's Wasm entrypoint exports.
use cosmwasm_std::{
    entry_point, from_json, to_json_binary, Addr, BankMsg, Binary, Deps, DepsMut, Env, MessageInfo,
    Response, StdError, StdResult, Uint128, Uint256, WasmMsg,
};
use dca_example_protocol::{
    DcaCallbackMsg, ExchangeConfig, ExchangeExecuteMsg, ExchangeQueryMsg, SwapReceipt,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct InstantiateMsg {
    pub input_denom: String,
    pub output_denom: String,
    pub numerator: Uint128,
    pub denominator: Uint128,
    pub mode: String,
}

fn receipt_key(requester: &Addr, order: u64, slice: u32) -> Vec<u8> {
    format!("fault/receipt/{requester}/{order}/{slice}").into_bytes()
}

#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    _: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> StdResult<Response> {
    if !info.funds.is_empty()
        || !matches!(
            msg.mode.as_str(),
            "omitted" | "invalid" | "duplicate" | "bank_failure"
        )
        || msg.numerator.is_zero()
        || msg.denominator.is_zero()
    {
        return Err(StdError::generic_err("invalid fault fixture configuration"));
    }
    deps.storage.set(
        b"fault/config",
        &to_json_binary(&ExchangeConfig {
            input_denom: msg.input_denom,
            output_denom: msg.output_denom,
            numerator: msg.numerator,
            denominator: msg.denominator,
        })?,
    );
    deps.storage.set(b"fault/mode", msg.mode.as_bytes());
    Ok(Response::new())
}

#[entry_point]
pub fn execute(
    deps: DepsMut,
    _: Env,
    info: MessageInfo,
    msg: ExchangeExecuteMsg,
) -> StdResult<Response> {
    let ExchangeExecuteMsg::Swap(request) = msg;
    let config: ExchangeConfig = from_json(deps.storage.get(b"fault/config").unwrap())?;
    let key = receipt_key(&info.sender, request.order_id, request.slice_index);
    if info.funds != [request.input.clone()] || deps.storage.get(&key).is_some() {
        return Err(StdError::generic_err("invalid or duplicate fault swap"));
    }
    let amount = Uint128::try_from(
        Uint256::from(request.input.amount)
            .checked_mul(Uint256::from(config.numerator))?
            .checked_div(Uint256::from(config.denominator))?,
    )?;
    let beneficiary = deps.api.addr_validate(&request.beneficiary)?;
    let mut receipt = SwapReceipt {
        order_id: request.order_id,
        slice_index: request.slice_index,
        correlation: request.correlation,
        input: request.input,
        output: cosmwasm_std::Coin {
            denom: config.output_denom,
            amount,
        },
        beneficiary: beneficiary.to_string(),
    };
    deps.storage.set(&key, &to_json_binary(&receipt)?);
    let mode = deps.storage.get(b"fault/mode").unwrap();
    let mut response = Response::new().add_message(BankMsg::Send {
        to_address: beneficiary.to_string(),
        amount: vec![if mode == b"bank_failure" {
            cosmwasm_std::Coin {
                amount: Uint128::MAX,
                ..receipt.output.clone()
            }
        } else {
            receipt.output.clone()
        }],
    });
    if mode == b"omitted" {
        return Ok(response);
    }
    if mode == b"invalid" {
        receipt.input.amount = receipt.input.amount.checked_add(Uint128::one())?;
    }
    let callback = WasmMsg::Execute {
        contract_addr: info.sender.to_string(),
        msg: to_json_binary(&DcaCallbackMsg::SwapResult { receipt })?,
        funds: vec![],
    };
    response = response.add_message(callback.clone());
    if mode == b"duplicate" {
        response = response.add_message(callback);
    }
    Ok(response)
}

#[entry_point]
pub fn query(deps: Deps, _: Env, msg: ExchangeQueryMsg) -> StdResult<Binary> {
    match msg {
        ExchangeQueryMsg::Config {} => Ok(Binary::from(deps.storage.get(b"fault/config").unwrap())),
        ExchangeQueryMsg::Receipt {
            dca,
            order_id,
            slice_index,
        } => {
            let requester = deps.api.addr_validate(&dca)?;
            let receipt = deps
                .storage
                .get(&receipt_key(&requester, order_id, slice_index))
                .map(from_json::<SwapReceipt>)
                .transpose()?;
            to_json_binary(&receipt)
        }
    }
}
