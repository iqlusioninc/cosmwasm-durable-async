use crate::{error::ContractError, state};
use cosmwasm_std::{
    entry_point, to_json_binary, BankMsg, Binary, Coin, Deps, DepsMut, Env, MessageInfo, Response,
    StdResult, Uint128, Uint256, WasmMsg,
};
use dca_example_protocol::{
    DcaCallbackMsg, ExchangeConfig, ExchangeExecuteMsg, ExchangeQueryMsg, SwapReceipt, SwapRequest,
    MAX_RESULT_BYTES,
};

#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: ExchangeConfig,
) -> Result<Response, ContractError> {
    if !info.funds.is_empty() {
        return Err(ContractError::UnexpectedFunds);
    }
    validate_denom(&msg.input_denom)?;
    validate_denom(&msg.output_denom)?;
    if msg.input_denom == msg.output_denom {
        return Err(ContractError::SameDenom);
    }
    if msg.numerator.is_zero() || msg.denominator.is_zero() {
        return Err(ContractError::InvalidRatio);
    }
    state::save_config(deps.storage, &msg)?;
    Ok(Response::new())
}

#[entry_point]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExchangeExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExchangeExecuteMsg::Swap(request) => swap(deps, env, info, request),
    }
}

fn swap(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    request: SwapRequest,
) -> Result<Response, ContractError> {
    let config = state::load_config(deps.storage)?;
    if request.input.amount.is_zero() {
        return Err(ContractError::ZeroInput);
    }
    if request.minimum_output.is_zero() {
        return Err(ContractError::ZeroMinimum);
    }
    if request.input.denom != config.input_denom {
        return Err(ContractError::InputDenom);
    }
    if request.output_denom != config.output_denom {
        return Err(ContractError::OutputDenom);
    }
    if info.funds.as_slice() != [request.input.clone()] {
        return Err(ContractError::ExactFunds);
    }
    let beneficiary = deps.api.addr_validate(&request.beneficiary)?;
    if state::load_receipt(
        deps.storage,
        &info.sender,
        request.order_id,
        request.slice_index,
    )?
    .is_some()
    {
        return Err(ContractError::DuplicateSwap);
    }
    let amount = quote(request.input.amount, config.numerator, config.denominator)?;
    if amount < request.minimum_output {
        return Err(ContractError::MinimumOutput);
    }
    let output = Coin {
        denom: config.output_denom,
        amount,
    };
    if deps
        .querier
        .query_balance(&env.contract.address, &output.denom)?
        .amount
        < amount
    {
        return Err(ContractError::InsufficientLiquidity);
    }
    let receipt = SwapReceipt {
        order_id: request.order_id,
        slice_index: request.slice_index,
        correlation: request.correlation,
        input: request.input,
        output: output.clone(),
        beneficiary: beneficiary.to_string(),
    };
    let encoded = encode_receipt(&receipt)?;
    let callback = to_json_binary(&DcaCallbackMsg::SwapResult {
        receipt: receipt.clone(),
    })?;
    state::save_receipt(deps.storage, &info.sender, &receipt, encoded.as_slice());
    // Ordinary messages preserve order and propagate errors through the enclosing transaction.
    Ok(Response::new()
        .add_message(BankMsg::Send {
            to_address: beneficiary.to_string(),
            amount: vec![output],
        })
        .add_message(WasmMsg::Execute {
            contract_addr: info.sender.to_string(),
            msg: callback,
            funds: vec![],
        }))
}

#[entry_point]
pub fn query(deps: Deps, _env: Env, msg: ExchangeQueryMsg) -> StdResult<Binary> {
    match msg {
        ExchangeQueryMsg::Config {} => to_json_binary(&state::load_config(deps.storage)?),
        ExchangeQueryMsg::Receipt {
            dca,
            order_id,
            slice_index,
        } => {
            let requester = deps.api.addr_validate(&dca)?;
            to_json_binary(&state::load_receipt(
                deps.storage,
                &requester,
                order_id,
                slice_index,
            )?)
        }
    }
}

fn quote(
    input: Uint128,
    numerator: Uint128,
    denominator: Uint128,
) -> Result<Uint128, ContractError> {
    if numerator.is_zero() || denominator.is_zero() {
        return Err(ContractError::InvalidRatio);
    }
    let wide = Uint256::from(input)
        .checked_mul(Uint256::from(numerator))
        .map_err(|_| ContractError::QuoteOverflow)?
        .checked_div(Uint256::from(denominator))
        .map_err(|_| ContractError::InvalidRatio)?;
    let output = Uint128::try_from(wide).map_err(|_| ContractError::QuoteOverflow)?;
    if output.is_zero() {
        return Err(ContractError::ZeroOutput);
    }
    Ok(output)
}

fn encode_receipt(receipt: &SwapReceipt) -> Result<Binary, ContractError> {
    let encoded = to_json_binary(receipt)?;
    if encoded.len() > MAX_RESULT_BYTES as usize {
        return Err(ContractError::ReceiptTooLarge);
    }
    Ok(encoded)
}

fn validate_denom(denom: &str) -> Result<(), ContractError> {
    // Cosmos SDK's default native denomination grammar: [a-zA-Z][a-zA-Z0-9/:._-]{2,127}.
    if !(3..=128).contains(&denom.len())
        || !denom.as_bytes()[0].is_ascii_alphabetic()
        || !denom
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/:._-".contains(&b))
    {
        return Err(ContractError::InvalidDenom);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::{from_json, Uint128};

    #[test]
    fn checked_wide_quote_rejects_zero_and_narrowing_overflow() {
        assert_eq!(
            quote(10u128.into(), 3u128.into(), 2u128.into()).unwrap(),
            Uint128::new(15)
        );
        assert!(quote(1u128.into(), 1u128.into(), 3u128.into()).is_err());
        assert!(quote(Uint128::MAX, Uint128::MAX, Uint128::one()).is_err());
        assert_eq!(
            quote(Uint128::MAX, Uint128::MAX, Uint128::MAX).unwrap(),
            Uint128::MAX
        );
        assert!(quote(Uint128::one(), Uint128::one(), Uint128::zero()).is_err());
    }

    #[test]
    fn receipt_bound_counts_encoded_escaped_bytes() {
        let receipt: SwapReceipt = from_json(include_bytes!(
            "../../dca-protocol/tests/fixtures/receipt-2048.json"
        ))
        .unwrap();
        assert_eq!(encode_receipt(&receipt).unwrap().len(), 2048);
        let receipt: SwapReceipt = from_json(include_bytes!(
            "../../dca-protocol/tests/fixtures/receipt-2049.json"
        ))
        .unwrap();
        assert!(encode_receipt(&receipt).is_err());
    }
}
