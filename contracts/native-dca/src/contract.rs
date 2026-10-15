use crate::{error::ContractError, msg::*, state::*};
use cosmwasm_std::*;
use dca_example_protocol::{ExchangeConfig, ExchangeQueryMsg, MAX_RESULT_BYTES, MAX_SLICES};
use native_async::{
    RuntimeMsg, RuntimeQuery, SchedulerQuery, TaskProgram, TaskResponse, TaskStatus,
};
use std::collections::BTreeMap;

pub const REGISTER_REPLY: u64 = 1;
/// Admission policy: a 4 KiB raw continuation envelope plus host wrappers.
/// Base64 expands 4,096 bytes to 5,464; the remaining 10,920 bytes cover
/// maximum-width task/funding wrappers and bounded terminal diagnostics.
/// Task 5 must test every actual generated wait state against this envelope.
pub const MAX_CHECKPOINT_BYTES: usize = 4_096;
pub const STATE_AND_HOST_ALLOWANCE: u64 = 16_384;
pub const MIN_ALLOCATED_BYTES: u64 = native_async::METADATA_ALLOWANCE
    + native_async::inbox_reserve(MAX_RESULT_BYTES)
    + STATE_AND_HOST_ALLOWANCE;

#[entry_point]
pub fn instantiate(
    deps: DepsMut<RuntimeQuery>,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response<RuntimeMsg>, ContractError> {
    no_funds(&info)?;
    let exchange = deps.api.addr_validate(&msg.exchange)?;
    let terms: ExchangeConfig = deps
        .querier
        .query_wasm_smart(&exchange, &ExchangeQueryMsg::Config {})?;
    let host: native_async::FundingConfig =
        deps.querier
            .query(&QueryRequest::Custom(RuntimeQuery::Funding(
                SchedulerQuery::FundingConfig {},
            )))?;
    if !valid_denom(&msg.fee_denom)
        || msg.fee_denom != host.denom
        || !valid_denom(&terms.input_denom)
        || !valid_denom(&terms.output_denom)
        || terms.input_denom == terms.output_denom
        || terms.numerator.is_zero()
        || terms.denominator.is_zero()
    {
        return Err(ContractError::InvalidConfig);
    }
    deps.storage.set(
        CONFIG_KEY,
        &to_json_vec(&Config {
            exchange,
            fee_denom: msg.fee_denom,
            input_denom: terms.input_denom,
            output_denom: terms.output_denom,
        })?,
    );
    Ok(Response::new())
}
#[entry_point]
pub fn execute(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response<RuntimeMsg>, ContractError> {
    match msg {
        ExecuteMsg::CreateOrder(request) => create_order(deps, env, info, request),
        ExecuteMsg::Async(msg) => {
            let config = load_config(deps.storage)?;
            Ok(native_async::execute_async(&info, msg, &config.fee_denom)?)
        }
        ExecuteMsg::UpdateBudget {
            order_id,
            gas_per_poll,
            max_gas_price,
        } => {
            no_funds(&info)?;
            let order = load_order(deps.storage, order_id)?;
            owner_only(&info, &order)?;
            if gas_per_poll == 0 || max_gas_price.is_zero() {
                return Err(ContractError::InvalidOrder);
            }
            Ok(native_async::update_budget(
                &info,
                bound_task(&order)?,
                gas_per_poll,
                max_gas_price,
                order.owner.to_string(),
            )?)
        }
        ExecuteMsg::ReclaimPrincipal { order_id } => reclaim(deps, env, info, order_id),
        ExecuteMsg::DispatchSlice {
            order_id,
            slice_index,
            correlation,
        } => crate::settlement::dispatch_slice(deps, env, info, order_id, slice_index, correlation),
        ExecuteMsg::SwapResult { receipt } => {
            crate::settlement::swap_result(deps, env, info, receipt)
        }
    }
}
#[entry_point]
pub fn reply(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    reply: Reply,
) -> Result<Response<RuntimeMsg>, ContractError> {
    if reply.id == crate::settlement::DISPATCH_REPLY {
        return crate::settlement::dispatch_reply(deps, env, reply);
    }
    if reply.id != REGISTER_REPLY {
        return Err(ContractError::InvalidReply);
    }
    let payload: RegisterReplyPayload =
        from_json(&reply.payload).map_err(|_| ContractError::InvalidReply)?;
    let mut order = load_order(deps.storage, payload.order_id)?;
    if order.task_id.is_some() {
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
    let task = host_task(deps.as_ref(), &env, task_id)?;
    if task.contract != env.contract.address.as_str()
        || task.task_id != task_id
        || task.program != program(order.order_id)?
        || task.expires_at != order.expires_at
    {
        return Err(ContractError::InvalidReply);
    }
    order.task_id = Some(task_id);
    save_order(deps.storage, &order)?;
    Ok(
        Response::new().set_data(to_json_binary(&CreateOrderResponse {
            order_id: order.order_id,
            task_id,
        })?),
    )
}

/// Privileged host-only workflow entrypoint. Never routed through execute.
#[entry_point]
pub fn sudo(
    deps: DepsMut,
    env: Env,
    msg: native_async::TaskSudoMsg,
) -> Result<Response, ContractError> {
    let native_async::TaskSudoMsg::PollTask(request) = msg;
    let registry = durable_runtime::Registry::new(vec![durable_runtime::registration::<
        crate::workflow::native_dca::Workflow,
    >()])?;
    Ok(durable_runtime::native::poll(
        deps,
        env,
        request,
        &registry,
        MAX_RESULT_BYTES,
    )?)
}
#[entry_point]
pub fn query(deps: Deps<RuntimeQuery>, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Order { order_id } => {
            let order = load_order(deps.storage, order_id)?;
            let host = match bound_task(&order).and_then(|task_id| host_task(deps, &env, task_id)) {
                Ok(task) => HostTaskView::Present(task),
                Err(error) => HostTaskView::Unavailable {
                    error: error.to_string(),
                    lease_expired: env.block.height >= order.expires_at,
                },
            };
            to_json_binary(&OrderView { order, host })
        }
        QueryMsg::Fill {
            order_id,
            slice_index,
        } => to_json_binary(&load_fill(deps.storage, order_id, slice_index)?),
        QueryMsg::Task { order_id } => to_json_binary(&host_task(
            deps,
            &env,
            bound_task(&load_order(deps.storage, order_id)?)?,
        )?),
        QueryMsg::Async(native_async::AsyncQueryMsg::FundingConfig {}) => {
            to_json_binary(&deps.querier.query::<native_async::FundingConfig>(
                &QueryRequest::Custom(RuntimeQuery::Funding(SchedulerQuery::FundingConfig {})),
            )?)
        }
        QueryMsg::Async(native_async::AsyncQueryMsg::TaskFunding { task_id }) => {
            to_json_binary(&deps.querier.query::<native_async::TaskFunding>(
                &QueryRequest::Custom(RuntimeQuery::Funding(SchedulerQuery::TaskFunding {
                    contract: env.contract.address.to_string(),
                    task_id,
                })),
            )?)
        }
    }
}

fn create_order(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    info: MessageInfo,
    request: CreateOrderRequest,
) -> Result<Response<RuntimeMsg>, ContractError> {
    let config = load_config(deps.storage)?;
    if !(1..=MAX_SLICES).contains(&request.slice_count)
        || request.input_per_slice.amount.is_zero()
        || request.input_per_slice.denom != config.input_denom
        || request.minimum_output.is_zero()
        || request.interval_blocks == 0
        || request.gas.amount.is_zero()
        || request.gas.denom != config.fee_denom
        || request.gas_per_poll == 0
        || request.max_gas_price.is_zero()
        || request.expires_at <= env.block.height
    {
        return Err(ContractError::InvalidOrder);
    }
    if request.allocated_bytes < MIN_ALLOCATED_BYTES {
        return Err(ContractError::Allocation);
    }
    let beneficiary = deps.api.addr_validate(&request.beneficiary)?;
    let principal = request
        .input_per_slice
        .amount
        .checked_mul(Uint128::from(request.slice_count))
        .map_err(StdError::overflow)?;
    exact_funds(
        &info,
        &request.input_per_slice.denom,
        principal,
        &request.gas,
    )?;
    let order_id: u64 = deps
        .storage
        .get(NEXT_ORDER_ID_KEY)
        .map(from_json)
        .transpose()?
        .unwrap_or(1);
    let next = order_id
        .checked_add(1)
        .ok_or_else(|| StdError::generic_err("order ID overflow"))?;
    let order = OrderRecord {
        order_id,
        task_id: None,
        owner: info.sender.clone(),
        beneficiary,
        exchange: config.exchange,
        input_per_slice: request.input_per_slice,
        output_denom: config.output_denom,
        slice_count: request.slice_count,
        filled_count: 0,
        minimum_output: request.minimum_output,
        initial_principal: principal,
        remaining: principal,
        reclaimed: Uint128::zero(),
        spent: Uint128::zero(),
        received: Uint128::zero(),
        interval_blocks: request.interval_blocks,
        registration_height: env.block.height,
        expires_at: request.expires_at,
        pending: None,
        last_fill_height: None,
    };
    save_order(deps.storage, &order)?;
    deps.storage.set(NEXT_ORDER_ID_KEY, &to_json_vec(&next)?);
    Ok(Response::new().add_submessage(
        SubMsg::reply_on_success(
            CosmosMsg::Custom(RuntimeMsg::Register {
                control: info.sender.to_string(),
                refund: info.sender.to_string(),
                gas_per_poll: request.gas_per_poll,
                max_gas_price: request.max_gas_price,
                expires_at: request.expires_at,
                funds: request.gas,
                program: program(order_id)?,
                allocated_bytes: request.allocated_bytes,
            }),
            REGISTER_REPLY,
        )
        .with_payload(to_json_binary(&RegisterReplyPayload { order_id })?),
    ))
}

fn reclaim(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    info: MessageInfo,
    order_id: u64,
) -> Result<Response<RuntimeMsg>, ContractError> {
    no_funds(&info)?;
    let mut order = load_order(deps.storage, order_id)?;
    owner_only(&info, &order)?;
    if order.remaining.is_zero() {
        return Err(ContractError::EmptyPrincipal);
    }
    if env.block.height < order.expires_at {
        let status = host_task(deps.as_ref(), &env, bound_task(&order)?)?.status;
        if !matches!(
            status,
            TaskStatus::Completed
                | TaskStatus::Failed
                | TaskStatus::Cancelled
                | TaskStatus::Expired
        ) {
            return Err(ContractError::NotTerminal);
        }
    }
    let amount = order.remaining;
    order.reclaimed = order
        .reclaimed
        .checked_add(amount)
        .map_err(StdError::overflow)?;
    order.remaining = Uint128::zero();
    save_order(deps.storage, &order)?;
    Ok(Response::new().add_message(BankMsg::Send {
        to_address: order.owner.to_string(),
        amount: vec![Coin {
            denom: order.input_per_slice.denom,
            amount,
        }],
    }))
}

fn program(order_id: u64) -> StdResult<TaskProgram> {
    Ok(TaskProgram {
        kind: "native_dca".into(),
        version: 1,
        input: to_json_binary(&WorkflowInput { order_id })?,
    })
}
fn host_task(deps: Deps<RuntimeQuery>, env: &Env, task_id: u64) -> StdResult<TaskResponse> {
    deps.querier
        .query(&QueryRequest::Custom(RuntimeQuery::Task {
            contract: env.contract.address.to_string(),
            task_id,
        }))
}
fn bound_task(order: &OrderRecord) -> StdResult<u64> {
    order
        .task_id
        .ok_or_else(|| StdError::generic_err("order registration is not bound"))
}
fn no_funds(info: &MessageInfo) -> Result<(), ContractError> {
    if info.funds.is_empty() {
        Ok(())
    } else {
        Err(ContractError::UnexpectedFunds)
    }
}
fn owner_only(info: &MessageInfo, order: &OrderRecord) -> Result<(), ContractError> {
    if info.sender == order.owner {
        Ok(())
    } else {
        Err(ContractError::Unauthorized)
    }
}
fn exact_funds(
    info: &MessageInfo,
    input_denom: &str,
    principal: Uint128,
    gas: &Coin,
) -> Result<(), ContractError> {
    let mut expected = BTreeMap::from([(input_denom.to_string(), principal)]);
    let initial = expected.entry(gas.denom.clone()).or_insert(Uint128::zero());
    *initial = initial
        .checked_add(gas.amount)
        .map_err(StdError::overflow)?;
    let mut attached = BTreeMap::new();
    for coin in &info.funds {
        if coin.amount.is_zero() || attached.insert(coin.denom.clone(), coin.amount).is_some() {
            return Err(ContractError::ExactFunds);
        }
    }
    if attached != expected {
        return Err(ContractError::ExactFunds);
    }
    Ok(())
}
fn valid_denom(denom: &str) -> bool {
    (3..=128).contains(&denom.len())
        && denom.as_bytes()[0].is_ascii_alphabetic()
        && denom
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/:._-".contains(&byte))
}
