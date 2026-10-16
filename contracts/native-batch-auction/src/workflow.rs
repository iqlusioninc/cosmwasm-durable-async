//! Generated captures contain only the auction identity and small progress values.
use crate::{
    error::ContractError,
    settlement::{advance_batch, BatchProgress},
    state::*,
};
use durable_macro::durable_workflow;
use durable_runtime::{native::SleepBlocks, WaitError, WorkflowCtx};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AuctionOutcome {
    Finalized,
    LeaseWindowExhausted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Error)]
pub enum AuctionWorkflowError {
    #[error("auction settlement invariant")]
    Invariant,
    #[error("auction storage unavailable")]
    Storage,
    #[error("unexpected auction timer result")]
    UnexpectedWait,
}
impl From<WaitError> for AuctionWorkflowError {
    fn from(_: WaitError) -> Self {
        Self::UnexpectedWait
    }
}
impl From<cosmwasm_std::StdError> for AuctionWorkflowError {
    fn from(_: cosmwasm_std::StdError) -> Self {
        Self::Storage
    }
}
impl From<ContractError> for AuctionWorkflowError {
    fn from(error: ContractError) -> Self {
        match error {
            ContractError::Std(_) => Self::Storage,
            _ => Self::Invariant,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StepStatus {
    Continue,
    Finalized,
    LeaseWindowExhausted,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepProgress {
    pub status: StepStatus,
    pub sleep_blocks: u64,
}

pub fn step(
    ctx: &mut WorkflowCtx<'_>,
    auction_id: u64,
) -> Result<StepProgress, AuctionWorkflowError> {
    let a = load_auction(ctx.storage())?;
    let height = ctx.env().block.height;
    let task_id = ctx.correlation().workflow_id;
    if a.auction_id != auction_id || a.task_id != Some(task_id) || a.state != BusinessState::Active
    {
        return Err(AuctionWorkflowError::Invariant);
    }
    if height >= a.terms.expires_at {
        return Ok(StepProgress {
            status: StepStatus::LeaseWindowExhausted,
            sleep_blocks: 0,
        });
    }
    if height < a.terms.reveal_end {
        return Ok(StepProgress {
            status: StepStatus::Continue,
            sleep_blocks: a.terms.reveal_end - height,
        });
    }
    let progress = advance_batch(ctx.storage(), height, task_id)?;
    let status = match progress {
        BatchProgress::Finalized => StepStatus::Finalized,
        BatchProgress::LeaseWindowExhausted => StepStatus::LeaseWindowExhausted,
        BatchProgress::Continue
            if height
                .checked_add(1)
                .is_some_and(|next| next < a.terms.expires_at) =>
        {
            StepStatus::Continue
        }
        BatchProgress::Continue => StepStatus::LeaseWindowExhausted,
    };
    let sleep_blocks = u64::from(status == StepStatus::Continue);
    Ok(StepProgress {
        status,
        sleep_blocks,
    })
}

#[durable_workflow(kind = "native_batch_auction", version = 1)]
pub async fn native_batch_auction(
    ctx: WorkflowCtx,
    auction_id: u64,
) -> Result<AuctionOutcome, AuctionWorkflowError> {
    let mut current: StepProgress = step(ctx, auction_id)?;
    while current.status == StepStatus::Continue {
        let timer: () = ctx
            .wait::<SleepBlocks>(current.sleep_blocks)
            .checkpoint(auction_id, current)
            .await?;
        current = step(ctx, auction_id)?;
    }
    Ok(if current.status == StepStatus::Finalized {
        AuctionOutcome::Finalized
    } else {
        AuctionOutcome::LeaseWindowExhausted
    })
}
