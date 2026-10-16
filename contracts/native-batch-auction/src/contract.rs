use crate::{admission, claims, error::ContractError, msg::*, queries, state::*};
use cosmwasm_std::*;
use native_async::{AsyncExecuteMsg, RuntimeMsg, RuntimeQuery, TaskProgram};
use serde::{Deserialize, Serialize};

pub const REGISTER_REPLY: u64 = 1;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegisterReplyPayload {
    pub auction_id: u64,
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut<RuntimeQuery>,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response<RuntimeMsg>, ContractError> {
    no_funds(&info)?;
    let host: native_async::FundingConfig =
        deps.querier
            .query(&QueryRequest::Custom(RuntimeQuery::Funding(
                native_async::SchedulerQuery::FundingConfig {},
            )))?;
    if !valid_denom(&msg.sale_denom)
        || !valid_denom(&msg.quote_denom)
        || !valid_denom(&msg.fee_denom)
        || msg.sale_denom == msg.quote_denom
        || msg.fee_denom != host.denom
    {
        return Err(ContractError::InvalidConfig);
    }
    let seller = canonical_address(deps.api, info.sender.as_str())?;
    deps.storage.set(
        CONFIG_KEY,
        &to_json_vec(&Config {
            seller,
            sale_denom: msg.sale_denom,
            quote_denom: msg.quote_denom,
            fee_denom: msg.fee_denom,
        })?,
    );
    Ok(Response::new())
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response<RuntimeMsg>, ContractError> {
    if !matches!(
        &msg,
        ExecuteMsg::CreateAuction(_)
            | ExecuteMsg::CommitBid { .. }
            | ExecuteMsg::Async(AsyncExecuteMsg::FundTask { .. })
    ) {
        no_funds(&info)?;
    }
    match msg {
        ExecuteMsg::CreateAuction(r) => admission::create(deps, env, info, r),
        ExecuteMsg::CommitBid { digest, collateral } => {
            admission::commit(deps, env, info, digest, collateral)
        }
        ExecuteMsg::RevealBid(r) => admission::reveal(deps, env, info, r),
        ExecuteMsg::ClaimBid { bidder } => claims::bid(deps, env, bidder),
        ExecuteMsg::ClaimSeller {} => claims::seller(deps, env),
        ExecuteMsg::Abort {} => claims::abort(deps, env),
        ExecuteMsg::Async(msg) => controls(deps, env, info, msg),
        ExecuteMsg::UpdateBudget {
            gas_per_poll,
            max_gas_price,
        } => {
            let config = load_config(deps.storage)?;
            seller_only(&info, &config)?;
            let mut auction = load_auction(deps.storage)?;
            active_before_expiry(&auction, env.block.height)?;
            if gas_per_poll == 0 || max_gas_price.is_zero() {
                return Err(ContractError::InvalidTerms);
            }
            if auction.commit_count > 0
                && (gas_per_poll < auction.budget.gas_per_poll
                    || max_gas_price < auction.budget.max_gas_price)
            {
                return Err(ContractError::BudgetReduction);
            }
            let response = native_async::update_budget(
                &info,
                bound_task(&auction)?,
                gas_per_poll,
                max_gas_price,
                config.seller.to_string(),
            )?;
            auction.budget = Budget {
                gas_per_poll,
                max_gas_price,
            };
            save_auction(deps.storage, &auction)?;
            Ok(response)
        }
    }
}
fn controls(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    info: MessageInfo,
    msg: AsyncExecuteMsg,
) -> Result<Response<RuntimeMsg>, ContractError> {
    let config = load_config(deps.storage)?;
    let mut auction = load_auction(deps.storage)?;
    let supplied = match msg {
        AsyncExecuteMsg::FundTask { task_id }
        | AsyncExecuteMsg::ResumeTask { task_id }
        | AsyncExecuteMsg::CancelTask { task_id } => task_id,
    };
    if supplied != bound_task(&auction)? {
        return Err(ContractError::InvalidTask);
    }
    if !matches!(msg, AsyncExecuteMsg::FundTask { .. }) {
        seller_only(&info, &config)?;
        active_before_expiry(&auction, env.block.height)?;
        if matches!(msg, AsyncExecuteMsg::CancelTask { .. }) {
            if auction.commit_count != 0 {
                return Err(ContractError::HasCommitments);
            }
            auction.state = BusinessState::Aborted;
            save_auction(deps.storage, &auction)?;
        }
    }
    // Host failure rolls back the business-state write and native message together.
    Ok(native_async::execute_async(&info, msg, &config.fee_denom)?)
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn reply(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    reply: Reply,
) -> Result<Response<RuntimeMsg>, ContractError> {
    if reply.id != REGISTER_REPLY {
        return Err(ContractError::InvalidReply);
    }
    let payload: RegisterReplyPayload =
        from_json(reply.payload).map_err(|_| ContractError::InvalidReply)?;
    let mut auction = load_auction(deps.storage)?;
    if payload.auction_id != auction.auction_id || auction.task_id.is_some() {
        return Err(ContractError::InvalidReply);
    }
    let response = reply
        .result
        .into_result()
        .map_err(|_| ContractError::InvalidReply)?;
    #[allow(deprecated)]
    let task_id: u64 = from_json(response.data.ok_or(ContractError::InvalidReply)?)
        .map_err(|_| ContractError::InvalidReply)?;
    if task_id == 0 {
        return Err(ContractError::InvalidReply);
    }
    let task = queries::host_task(deps.as_ref(), &env, task_id)?;
    if task.contract != env.contract.address.as_str()
        || task.task_id != task_id
        || task.program != program(auction.auction_id)?
        || task.expires_at != auction.terms.expires_at
    {
        return Err(ContractError::InvalidReply);
    }
    auction.task_id = Some(task_id);
    save_auction(deps.storage, &auction)?;
    Ok(
        Response::new().set_data(to_json_binary(&CreateAuctionResponse {
            auction_id: auction.auction_id,
            task_id,
        })?),
    )
}
#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps<RuntimeQuery>, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    queries::query(deps, env, msg)
}
/// Host-only entry point. Application workflow errors must abort all poll writes.
#[cfg_attr(not(feature = "library"), entry_point)]
pub fn sudo(
    deps: DepsMut,
    env: Env,
    msg: native_async::TaskSudoMsg,
) -> Result<Response, ContractError> {
    let native_async::TaskSudoMsg::PollTask(request) = msg;
    let registry = durable_runtime::Registry::new(vec![durable_runtime::registration::<
        crate::workflow::native_batch_auction::Workflow,
    >()])
    .map_err(|e| StdError::generic_err(e.to_string()))?;
    let response =
        durable_runtime::native::poll(deps, env, request, &registry, crate::MAX_RESULT_BYTES)
            .map_err(|e| StdError::generic_err(e.to_string()))?;
    let poll: native_async::PollResponse = from_json(
        response
            .data
            .as_ref()
            .ok_or_else(|| StdError::generic_err("missing poll response"))?,
    )?;
    if matches!(poll.outcome, native_async::PollOutcome::Fail { .. }) {
        return Err(StdError::generic_err("auction workflow aborted").into());
    }
    Ok(response)
}
pub(crate) fn program(auction_id: u64) -> StdResult<TaskProgram> {
    Ok(TaskProgram {
        kind: "native_batch_auction".into(),
        version: 1,
        input: to_json_binary(&WorkflowInput { auction_id })?,
    })
}
pub(crate) fn no_funds(info: &MessageInfo) -> Result<(), ContractError> {
    if info.funds.is_empty() {
        Ok(())
    } else {
        Err(ContractError::UnexpectedFunds)
    }
}
pub(crate) fn seller_only(info: &MessageInfo, config: &Config) -> Result<(), ContractError> {
    if info.sender == config.seller {
        Ok(())
    } else {
        Err(ContractError::Unauthorized)
    }
}
pub(crate) fn bound_task(auction: &AuctionRecord) -> StdResult<u64> {
    auction
        .task_id
        .ok_or_else(|| StdError::generic_err("auction registration is not bound"))
}
pub(crate) fn active_before_expiry(
    auction: &AuctionRecord,
    height: u64,
) -> Result<(), ContractError> {
    if auction.state == BusinessState::Active && height < auction.terms.expires_at {
        Ok(())
    } else {
        Err(ContractError::InvalidPhase)
    }
}
pub(crate) fn canonical_address(api: &dyn Api, value: &str) -> StdResult<Addr> {
    let addr = api.addr_validate(value)?;
    if addr.as_str() != value {
        return Err(StdError::generic_err("noncanonical address"));
    }
    Ok(addr)
}
fn valid_denom(denom: &str) -> bool {
    (3..=128).contains(&denom.len())
        && denom.as_bytes()[0].is_ascii_alphabetic()
        && denom
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/:._-".contains(&b))
}
