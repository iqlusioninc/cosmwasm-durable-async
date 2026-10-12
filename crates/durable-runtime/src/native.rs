//! Stateless adapter for generated workflows. The native host owns checkpoints.
//! `WorkflowCtx` exposes ordinary CosmWasm queries only; native custom queries
//! belong in the contract's public query handler.
use crate::{
    Correlation, Deadline, Outcome, Registry, RuntimeError, RuntimeResult, Transition, WorkflowCtx,
};
use cosmwasm_std::{to_json_binary, DepsMut, Env, MessageInfo, Response};
use native_async::{PollOutcome, PollRequest, PollResponse, PollStep, Resolution, Wait};

/// Reserved operation identity. Other adapters must not use this identity.
pub const SLEEP_BLOCKS_KIND: &str = "native/sleep_blocks/v1";
/// Height timer authoring adapter; a positive block count resumes as unit.
pub struct SleepBlocks;
impl crate::Operation for SleepBlocks {
    const KIND: &'static str = SLEEP_BLOCKS_KIND;
    type Request = u64;
    type Output = ();
    fn prepare(
        ctx: &mut WorkflowCtx<'_>,
        _: Correlation,
        blocks: u64,
    ) -> RuntimeResult<crate::PreparedWait> {
        if blocks == 0 {
            return Err(RuntimeError::Validation("sleep must be positive".into()));
        }
        let height = ctx
            .env()
            .block
            .height
            .checked_add(blocks)
            .ok_or(RuntimeError::CounterOverflow)?;
        Ok(crate::PreparedWait {
            operation: Self::KIND.into(),
            request: to_json_binary(&blocks)?,
            resolver: ctx.env().contract.address.clone(),
            deadline: Deadline::Height(height),
            messages: vec![],
        })
    }
}

/// Called only from the contract's privileged sudo entry point. A contract must
/// never expose this operation through execute or accept caller-supplied polls.
pub fn poll(
    deps: DepsMut,
    env: Env,
    request: PollRequest,
    registry: &Registry,
    max_result_bytes: u32,
) -> RuntimeResult<Response> {
    let entry = registry.get(&request.kind, request.version)?;
    if max_result_bytes == 0 {
        return Err(RuntimeError::Validation("zero result bound".into()));
    }
    let next = request
        .wait_sequence
        .checked_add(1)
        .ok_or(RuntimeError::CounterOverflow)?;
    let info = MessageInfo {
        sender: env.contract.address.clone(),
        funds: vec![],
    };
    let mut ctx = WorkflowCtx {
        storage: deps.storage,
        api: deps.api,
        querier: deps.querier,
        env: &env,
        info: &info,
        correlation: Correlation {
            workflow_id: request.task_id,
            wait_sequence: next,
        },
    };
    let transition = match request.step {
        PollStep::Start { input } => {
            if request.wait_sequence != 0 {
                return Err(RuntimeError::Validation(
                    "start sequence must be zero".into(),
                ));
            }
            (entry.start)(&mut ctx, input)?
        }
        PollStep::Resume { state, resolution } => {
            if request.wait_sequence == 0 {
                return Err(RuntimeError::Validation(
                    "resume sequence must be positive".into(),
                ));
            }
            let timer = (entry.operation)(&state)? == SLEEP_BLOCKS_KIND;
            if timer && !matches!(resolution, Resolution::Timer {}) {
                return Err(RuntimeError::Validation(
                    "height timer requires timer resolution".into(),
                ));
            }
            let resolution = match resolution {
                Resolution::Success { data } => {
                    if data.len() > max_result_bytes as usize {
                        return Err(RuntimeError::LimitExceeded {
                            limit: "callback",
                            max: max_result_bytes as usize,
                            actual: data.len(),
                        });
                    }
                    crate::Resolution::Outcome(Outcome::Success(data))
                }
                Resolution::Error { code, message } => {
                    crate::Resolution::Outcome(Outcome::Error { code, message })
                }
                Resolution::Timeout {} => crate::Resolution::Timeout,
                Resolution::Timer {} if timer => {
                    crate::Resolution::Outcome(Outcome::Success(to_json_binary(&())?))
                }
                Resolution::Timer {} => {
                    return Err(RuntimeError::Validation(
                        "operation workflow cannot consume timer resolution".into(),
                    ))
                }
            };
            (entry.validate)(&state, &resolution)?;
            (entry.resume)(&mut ctx, state, resolution)?
        }
    };
    let (outcome, messages) = match transition {
        Transition::Wait { state, wait } => {
            let deadline = match wait.deadline {
                Deadline::Height(h) => h,
                Deadline::Time(_) => {
                    return Err(RuntimeError::Validation(
                        "native ABI does not support time deadlines".into(),
                    ))
                }
            };
            let native_wait = if wait.operation == SLEEP_BLOCKS_KIND {
                let blocks: u64 = cosmwasm_std::from_json(&wait.request)?;
                if blocks == 0
                    || !wait.messages.is_empty()
                    || wait.resolver != env.contract.address
                    || env.block.height.checked_add(blocks) != Some(deadline)
                {
                    return Err(RuntimeError::Validation(
                        "malformed reserved height timer".into(),
                    ));
                }
                Wait::Height { height: deadline }
            } else {
                Wait::Operation {
                    resolver: wait.resolver.to_string(),
                    deadline,
                    max_result_bytes,
                }
            };
            (
                PollOutcome::Pending {
                    state,
                    wait: native_wait,
                },
                wait.messages,
            )
        }
        Transition::Completed(output) => (PollOutcome::Complete { output }, vec![]),
        Transition::Failed(error) => (
            PollOutcome::Fail {
                error: String::from_utf8(error.to_vec())
                    .map_err(|e| RuntimeError::Validation(e.to_string()))?,
            },
            vec![],
        ),
    };
    Ok(Response::new().set_data(to_json_binary(&PollResponse { outcome, messages })?))
}
