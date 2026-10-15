//! Small generated continuations; business accounting remains authoritative in storage.
use crate::{msg::ExecuteMsg, state::load_order};
use cosmwasm_std::{to_json_binary, StdError, WasmMsg};
use dca_example_protocol::{Correlation, SwapReceipt};
use durable_macro::durable_workflow;
use durable_runtime::{
    native::SleepBlocks, Deadline, Operation, PreparedWait, RuntimeError, RuntimeResult, WaitError,
    WorkflowCtx,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DcaOutcome {
    Filled,
    LeaseWindowExhausted,
}

/// Fixed diagnostic categories keep remote strings out of terminal task storage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Error)]
pub enum DcaWorkflowError {
    #[error("unexpected swap timeout")]
    Timeout,
    #[error("unexpected remote swap error")]
    Remote,
    #[error("order progress unavailable")]
    Storage,
}
impl From<WaitError> for DcaWorkflowError {
    fn from(value: WaitError) -> Self {
        match value {
            WaitError::Timeout => Self::Timeout,
            WaitError::Remote { .. } => Self::Remote,
        }
    }
}
impl From<StdError> for DcaWorkflowError {
    fn from(_: StdError) -> Self {
        Self::Storage
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SliceRequest {
    pub order_id: u64,
    pub slice_index: u32,
}
pub struct SwapSlice;
impl Operation for SwapSlice {
    const KIND: &'static str = "native_dca/swap_slice/v1";
    type Request = SliceRequest;
    type Output = SwapReceipt;
    fn prepare(
        ctx: &mut WorkflowCtx<'_>,
        correlation: Correlation,
        request: SliceRequest,
    ) -> RuntimeResult<PreparedWait> {
        let order = load_order(ctx.storage(), request.order_id)?;
        let height = ctx.env().block.height;
        let deadline = height.checked_add(1).ok_or(RuntimeError::CounterOverflow)?;
        let due = order
            .last_fill_height
            .unwrap_or(order.registration_height)
            .checked_add(order.interval_blocks)
            .ok_or(RuntimeError::CounterOverflow)?;
        if order.task_id != Some(correlation.workflow_id)
            || request.slice_index != order.filled_count
            || order.filled_count >= order.slice_count
            || order.pending.is_some()
            || height < due
            || deadline >= order.expires_at
        {
            return Err(RuntimeError::Validation("invalid slice preparation".into()));
        }
        Ok(PreparedWait {
            operation: Self::KIND.into(),
            request: to_json_binary(&request)?,
            resolver: order.exchange,
            deadline: Deadline::Height(deadline),
            messages: vec![WasmMsg::Execute {
                contract_addr: ctx.env().contract.address.to_string(),
                msg: to_json_binary(&ExecuteMsg::DispatchSlice {
                    order_id: request.order_id,
                    slice_index: request.slice_index,
                    correlation,
                })?,
                funds: vec![],
            }
            .into()],
        })
    }
    // The default decoder only parses bytes. It is intentionally pure and is
    // invoked by generated validation and execution independently.
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Progress {
    slice_index: u32,
    sleep_blocks: u64,
    state: ProgressState,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum ProgressState {
    Ready,
    Filled,
    LeaseWindowExhausted,
}

/// Version-pinned view of the generated operation checkpoint. The hosts clear
/// registration input when staging a wait, so the checkpoint binds the order.
#[derive(Deserialize)]
enum DispatchCheckpoint {
    Waiting2 { order_id: u64, current: Progress },
}
pub(crate) fn matches_dispatch_checkpoint(
    state: &cosmwasm_std::Binary,
    order_id: u64,
    slice_index: u32,
) -> bool {
    match cosmwasm_std::from_json::<DispatchCheckpoint>(state) {
        Ok(DispatchCheckpoint::Waiting2 {
            order_id: saved,
            current,
        }) => {
            saved == order_id
                && current.slice_index == slice_index
                && current.sleep_blocks == 0
                && current.state == ProgressState::Ready
        }
        Err(_) => false,
    }
}

fn progress(ctx: &mut WorkflowCtx<'_>, order_id: u64) -> Result<Progress, DcaWorkflowError> {
    let order = load_order(ctx.storage(), order_id)?;
    let height = ctx.env().block.height;
    let mut value = Progress {
        slice_index: order.filled_count,
        sleep_blocks: 0,
        state: ProgressState::LeaseWindowExhausted,
    };
    if order.filled_count == order.slice_count {
        value.state = ProgressState::Filled;
    } else if let Some(due) = order
        .last_fill_height
        .unwrap_or(order.registration_height)
        .checked_add(order.interval_blocks)
    {
        if due < order.expires_at {
            if due > height {
                value.sleep_blocks = due - height;
                value.state = ProgressState::Ready;
            } else if height
                .checked_add(1)
                .is_some_and(|deadline| deadline < order.expires_at)
            {
                value.state = ProgressState::Ready;
            }
        }
    }
    Ok(value)
}

#[durable_workflow(kind = "native_dca", version = 1)]
pub async fn native_dca(ctx: WorkflowCtx, order_id: u64) -> Result<DcaOutcome, DcaWorkflowError> {
    let mut current: Progress = progress(ctx, order_id)?;
    while current.state == ProgressState::Ready {
        if current.sleep_blocks > 0 {
            let timer: () = ctx
                .wait::<SleepBlocks>(current.sleep_blocks)
                .checkpoint(order_id, current)
                .await?;
            current = progress(ctx, order_id)?;
        }
        if current.state == ProgressState::Ready {
            let receipt: SwapReceipt = ctx
                .wait::<SwapSlice>(SliceRequest {
                    order_id,
                    slice_index: current.slice_index,
                })
                .checkpoint(order_id, current)
                .await?;
            current = progress(ctx, order_id)?;
        }
    }
    Ok(if current.state == ProgressState::Filled {
        DcaOutcome::Filled
    } else {
        DcaOutcome::LeaseWindowExhausted
    })
}
