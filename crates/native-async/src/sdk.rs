use crate::{SchedulerMsg, TaskId};
use cosmwasm_std::{Coin, CosmosMsg, MessageInfo, Response, StdError, StdResult};

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

pub fn fund_task(
    info: &MessageInfo,
    task_id: TaskId,
    denom: &str,
) -> StdResult<Response<SchedulerMsg>> {
    Ok(
        Response::new().add_message(CosmosMsg::Custom(SchedulerMsg::FundTask {
            task_id,
            contributor: info.sender.to_string(),
            funds: attached_gas(info, denom)?,
        })),
    )
}

pub fn execute_async(
    info: &MessageInfo,
    msg: crate::AsyncExecuteMsg,
    denom: &str,
) -> StdResult<Response<SchedulerMsg>> {
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
    Ok(Response::new().add_message(CosmosMsg::Custom(msg)))
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
