use crate::{SchedulerMsg, TaskId};
use cosmwasm_std::{Coin, CosmosMsg, MessageInfo, Response, StdError, StdResult};

/// Forward the actual caller; the owning contract must never accept a supplied actor.
pub fn extend_lease(
    info: &MessageInfo,
    task_id: TaskId,
    allocated_bytes: u64,
    expires_at: u64,
) -> StdResult<Response<crate::RuntimeMsg>> {
    policy_message(
        info,
        crate::RuntimeMsg::ExtendLease {
            task_id,
            actor: info.sender.to_string(),
            allocated_bytes,
            expires_at,
        },
    )
}

/// Policy changes do not resume a paused task. Identical updates are accepted.
pub fn update_budget(
    info: &MessageInfo,
    task_id: TaskId,
    gas_per_poll: u64,
    max_gas_price: cosmwasm_std::Decimal,
    refund: String,
) -> StdResult<Response<crate::RuntimeMsg>> {
    policy_message(
        info,
        crate::RuntimeMsg::UpdateBudget {
            task_id,
            actor: info.sender.to_string(),
            gas_per_poll,
            max_gas_price,
            refund,
        },
    )
}

fn policy_message(
    info: &MessageInfo,
    msg: crate::RuntimeMsg,
) -> StdResult<Response<crate::RuntimeMsg>> {
    if !info.funds.is_empty() {
        return Err(StdError::generic_err(
            "management calls do not accept funds",
        ));
    }
    Ok(Response::new().add_message(CosmosMsg::Custom(msg)))
}

/// Allocation both hosts reserve for an operation wait's inbox: worst-case
/// escaped JSON of `max_result_bytes` plus framing. Hosts add a fixed
/// [`METADATA_ALLOWANCE`] on top, so a task waiting on such an operation needs
/// more than `METADATA_ALLOWANCE + inbox_reserve(max_result_bytes)` bytes.
pub const fn inbox_reserve(max_result_bytes: u32) -> u64 {
    max_result_bytes as u64 * 6 + 128
}

/// Fixed per-task allowance for counters, queue keys and diagnostics.
pub const METADATA_ALLOWANCE: u64 = 1024;

/// Accept only a dedicated gas deposit. Business funds must be split by the
/// application's start handler, not passed to this helper.
pub fn attached_gas(info: &MessageInfo, denom: &str) -> StdResult<Coin> {
    match info.funds.as_slice() {
        [coin] if coin.denom == denom && !coin.amount.is_zero() => Ok(coin.clone()),
        _ => Err(StdError::generic_err(
            "expected one positive gas coin in the configured denomination",
        )),
    }
}

/// Generic over the contract's custom message: `Response<RuntimeMsg>` wraps the
/// message in `RuntimeMsg::Funding` for executable tasks; `Response<SchedulerMsg>`
/// targets the legacy funding-only keeper.
pub fn fund_task<T: From<SchedulerMsg>>(
    info: &MessageInfo,
    task_id: TaskId,
    denom: &str,
) -> StdResult<Response<T>> {
    Ok(
        Response::new().add_message(CosmosMsg::Custom(T::from(SchedulerMsg::FundTask {
            task_id,
            contributor: info.sender.to_string(),
            funds: attached_gas(info, denom)?,
        }))),
    )
}

pub fn execute_async<T: From<SchedulerMsg>>(
    info: &MessageInfo,
    msg: crate::AsyncExecuteMsg,
    denom: &str,
) -> StdResult<Response<T>> {
    use crate::AsyncExecuteMsg;
    if let AsyncExecuteMsg::FundTask { task_id } = msg {
        return fund_task(info, task_id, denom);
    }
    if !info.funds.is_empty() {
        return Err(StdError::generic_err(
            "management calls do not accept funds",
        ));
    }
    let actor = info.sender.to_string();
    let msg = match msg {
        AsyncExecuteMsg::ResumeTask { task_id } => SchedulerMsg::ResumeTask { task_id, actor },
        AsyncExecuteMsg::CancelTask { task_id } => SchedulerMsg::CancelTask { task_id, actor },
        AsyncExecuteMsg::FundTask { .. } => unreachable!(),
    };
    Ok(Response::new().add_message(CosmosMsg::Custom(T::from(msg))))
}

pub fn query_funding(
    deps: cosmwasm_std::Deps<crate::SchedulerQuery>,
    env: &cosmwasm_std::Env,
    msg: crate::AsyncQueryMsg,
) -> StdResult<cosmwasm_std::Binary> {
    use crate::{AsyncQueryMsg, FundingConfig, SchedulerQuery, TaskFunding};
    use cosmwasm_std::{to_json_binary, QueryRequest};
    match msg {
        AsyncQueryMsg::FundingConfig {} => to_json_binary(
            &deps
                .querier
                .query::<FundingConfig>(&QueryRequest::Custom(SchedulerQuery::FundingConfig {}))?,
        ),
        AsyncQueryMsg::TaskFunding { task_id } => {
            to_json_binary(&deps.querier.query::<TaskFunding>(&QueryRequest::Custom(
                SchedulerQuery::TaskFunding {
                    contract: env.contract.address.to_string(),
                    task_id,
                },
            ))?)
        }
    }
}
