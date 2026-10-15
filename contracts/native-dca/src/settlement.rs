//! Transactional self-dispatch, authenticated receipt, and one-use reply guard.
use crate::{
    error::ContractError,
    msg::DispatchReplyPayload,
    state::*,
    workflow::{matches_dispatch_checkpoint, native_dca, SwapSlice},
};
use cosmwasm_std::*;
use dca_example_protocol::{
    Correlation, ExchangeExecuteMsg, SwapReceipt, SwapRequest, MAX_RESULT_BYTES,
};
use durable_runtime::{Operation, Workflow};
use native_async::{RuntimeMsg, RuntimeQuery, TaskResponse, TaskStatus, Wait};

pub const DISPATCH_REPLY: u64 = 2;
fn guard_key(order_id: u64) -> Vec<u8> {
    format!("dca/v1/dispatch-reply/{order_id:020}").into_bytes()
}
fn require(valid: bool) -> Result<(), ContractError> {
    if valid {
        Ok(())
    } else {
        Err(ContractError::Settlement)
    }
}
fn no_funds(info: &MessageInfo) -> Result<(), ContractError> {
    require(info.funds.is_empty())
}
fn matches_receipt(receipt: &SwapReceipt, request: &SwapRequest) -> bool {
    receipt.order_id == request.order_id
        && receipt.slice_index == request.slice_index
        && receipt.correlation == request.correlation
        && receipt.input == request.input
        && receipt.output.denom == request.output_denom
        && receipt.output.amount >= request.minimum_output
        && receipt.beneficiary == request.beneficiary
}
pub fn dispatch_slice(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    info: MessageInfo,
    order_id: u64,
    slice_index: u32,
    correlation: Correlation,
) -> Result<Response<RuntimeMsg>, ContractError> {
    no_funds(&info)?;
    require(info.sender == env.contract.address)?;
    let mut order = load_order(deps.storage, order_id)?;
    let height = env.block.height;
    let deadline = height.checked_add(1).ok_or(ContractError::Settlement)?;
    let due = order
        .last_fill_height
        .unwrap_or(order.registration_height)
        .checked_add(order.interval_blocks)
        .ok_or(ContractError::Settlement)?;
    require(
        order.task_id == Some(correlation.workflow_id)
            && correlation.wait_sequence > 0
            && order.filled_count == slice_index
            && slice_index < order.slice_count
            && order.pending.is_none()
            && deps.storage.get(&guard_key(order_id)).is_none()
            && load_fill(deps.storage, order_id, slice_index)?.is_none()
            && height >= due
            && deadline < order.expires_at,
    )?;
    let task: TaskResponse = deps
        .querier
        .query(&QueryRequest::Custom(RuntimeQuery::Task {
            contract: env.contract.address.to_string(),
            task_id: correlation.workflow_id,
        }))?;
    require(
        task.contract == env.contract.address.as_str()
            && task.task_id == correlation.workflow_id
            && task.program.kind == "native_dca"
            && task.program.version == 1
            && task.program.input.is_empty()
            && matches!(task.status, TaskStatus::Runnable | TaskStatus::Waiting)
            && task.wait_sequence == correlation.wait_sequence
            && task.expires_at == order.expires_at
            && task.last_attempt == Some(height)
            && task.inbox.is_none()
            && task.output.is_none()
            && task.error.is_none()
            && task.wait
                == Some(Wait::Operation {
                    resolver: order.exchange.to_string(),
                    deadline,
                    max_result_bytes: MAX_RESULT_BYTES,
                })
            && task.state.as_ref().is_some_and(|state| {
                matches_dispatch_checkpoint(state, order_id, slice_index)
                    && native_dca::Workflow::operation(state)
                        .is_ok_and(|kind| kind == SwapSlice::KIND)
            }),
    )?;
    require(
        order.initial_principal
            == order
                .spent
                .checked_add(order.remaining)
                .map_err(StdError::overflow)?
                .checked_add(order.reclaimed)
                .map_err(StdError::overflow)?,
    )?;
    order.remaining = order
        .remaining
        .checked_sub(order.input_per_slice.amount)
        .map_err(StdError::overflow)?;
    let request = SwapRequest {
        order_id,
        slice_index,
        correlation,
        input: order.input_per_slice.clone(),
        output_denom: order.output_denom.clone(),
        minimum_output: order.minimum_output,
        beneficiary: order.beneficiary.to_string(),
    };
    let pending = PendingDispatch {
        request: request.clone(),
        dispatch_height: height,
        deadline,
    };
    order.pending = Some(pending.clone());
    save_order(deps.storage, &order)?;
    deps.storage
        .set(&guard_key(order_id), &to_json_vec(&pending)?);
    Ok(Response::new().add_submessage(
        SubMsg::reply_on_success(
            WasmMsg::Execute {
                contract_addr: order.exchange.to_string(),
                msg: to_json_binary(&ExchangeExecuteMsg::Swap(request))?,
                funds: vec![order.input_per_slice],
            },
            DISPATCH_REPLY,
        )
        .with_payload(to_json_binary(&DispatchReplyPayload {
            order_id,
            slice_index,
            correlation,
        })?),
    ))
}

pub fn swap_result(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    info: MessageInfo,
    receipt: SwapReceipt,
) -> Result<Response<RuntimeMsg>, ContractError> {
    no_funds(&info)?;
    let mut order = load_order(deps.storage, receipt.order_id)?;
    let pending = order.pending.clone().ok_or(ContractError::Settlement)?;
    let encoded = to_json_binary(&receipt)?;
    require(
        info.sender == order.exchange
            && encoded.len() <= MAX_RESULT_BYTES as usize
            && matches_receipt(&receipt, &pending.request)
            && pending.dispatch_height == env.block.height
            && pending.deadline
                == env
                    .block
                    .height
                    .checked_add(1)
                    .ok_or(ContractError::Settlement)?
            && pending.deadline < order.expires_at
            && order.task_id == Some(receipt.correlation.workflow_id)
            && order.filled_count == receipt.slice_index
            && load_fill(deps.storage, receipt.order_id, receipt.slice_index)?.is_none(),
    )?;
    let guard: PendingDispatch = from_json(
        deps.storage
            .get(&guard_key(order.order_id))
            .ok_or(ContractError::Settlement)?,
    )?;
    require(guard == pending)?;
    order.spent = order
        .spent
        .checked_add(receipt.input.amount)
        .map_err(StdError::overflow)?;
    order.received = order
        .received
        .checked_add(receipt.output.amount)
        .map_err(StdError::overflow)?;
    order.filled_count = order
        .filled_count
        .checked_add(1)
        .ok_or(ContractError::Settlement)?;
    order.last_fill_height = Some(env.block.height);
    order.pending = None;
    require(
        order.filled_count <= order.slice_count
            && order.initial_principal
                == order
                    .spent
                    .checked_add(order.remaining)
                    .map_err(StdError::overflow)?
                    .checked_add(order.reclaimed)
                    .map_err(StdError::overflow)?,
    )?;
    save_fill(
        deps.storage,
        &FillRecord {
            receipt: receipt.clone(),
            height: env.block.height,
        },
    )?;
    save_order(deps.storage, &order)?;
    Ok(
        Response::new().add_message(CosmosMsg::Custom(RuntimeMsg::Deliver {
            task_id: receipt.correlation.workflow_id,
            wait_sequence: receipt.correlation.wait_sequence,
            resolver: info.sender.to_string(),
            resolution: native_async::Resolution::Success { data: encoded },
        })),
    )
}

pub fn dispatch_reply(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    reply: Reply,
) -> Result<Response<RuntimeMsg>, ContractError> {
    require(reply.id == DISPATCH_REPLY && reply.result.is_ok())?;
    let payload: DispatchReplyPayload =
        from_json(reply.payload).map_err(|_| ContractError::InvalidReply)?;
    let pending: PendingDispatch = from_json(
        deps.storage
            .get(&guard_key(payload.order_id))
            .ok_or(ContractError::InvalidReply)?,
    )?;
    let order = load_order(deps.storage, payload.order_id)?;
    let fill = load_fill(deps.storage, payload.order_id, payload.slice_index)?
        .ok_or(ContractError::InvalidReply)?;
    require(
        pending.request.order_id == payload.order_id
            && pending.request.slice_index == payload.slice_index
            && pending.request.correlation == payload.correlation
            && matches_receipt(&fill.receipt, &pending.request)
            && pending.dispatch_height == env.block.height
            && fill.height == pending.dispatch_height
            && order.pending.is_none()
            && order.task_id == Some(payload.correlation.workflow_id)
            && order.filled_count
                == payload
                    .slice_index
                    .checked_add(1)
                    .ok_or(ContractError::InvalidReply)?
            && order.last_fill_height == Some(fill.height),
    )?;
    deps.storage.remove(&guard_key(payload.order_id));
    Ok(Response::new())
}
