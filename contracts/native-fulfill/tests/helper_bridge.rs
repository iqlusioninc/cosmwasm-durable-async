use cosmwasm_std::{
    from_json,
    testing::{mock_dependencies, mock_env},
    to_json_binary, CosmosMsg, WasmMsg,
};
use durable_macro::durable_workflow;
use durable_runtime::{
    native::{poll, SleepBlocks},
    registration, Correlation, Deadline, Operation, PreparedWait, Registry, RuntimeResult,
    WaitError, WorkflowCtx,
};
use native_async::{PollOutcome, PollRequest, PollResponse, PollStep, Resolution, Wait};

struct Send;
impl Operation for Send {
    const KIND: &'static str = "helper-send";
    type Request = ();
    type Output = u32;
    fn prepare(ctx: &mut WorkflowCtx<'_>, key: Correlation, _: ()) -> RuntimeResult<PreparedWait> {
        Ok(PreparedWait {
            operation: Self::KIND.into(),
            request: to_json_binary(&())?,
            resolver: ctx.env().contract.address.clone(),
            deadline: Deadline::Height(ctx.env().block.height + 10),
            messages: vec![WasmMsg::Execute {
                contract_addr: ctx.env().contract.address.to_string(),
                msg: to_json_binary(&(key.workflow_id, key.wait_sequence))?,
                funds: vec![],
            }
            .into()],
        })
    }
}
#[durable_workflow(kind = "bridge-child", version = 1)]
async fn child(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let number: u32 = ctx.wait::<Send>(()).checkpoint().await?;
    let _done: () = ctx.wait::<SleepBlocks>(2).checkpoint(number).await?;
    Ok(number)
}
#[durable_workflow(kind = "bridge-parent", version = 1)]
async fn parent(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let number: u32 = ctx
        .call::<child::Workflow>(child::Input {})
        .checkpoint()
        .await?;
    let own: u32 = ctx.wait::<Send>(()).checkpoint(number).await?;
    Ok(number + own)
}

#[test]
fn helpers_bridge_child_operation_timer_and_parent_operation_with_one_host_sequence() {
    let mut deps = mock_dependencies();
    let mut env = mock_env();
    let registry = Registry::new(vec![registration::<parent::Workflow>()]).unwrap();
    let mut step = PollStep::Start {
        input: to_json_binary(&parent::Input {}).unwrap(),
    };
    for sequence in 0..=3 {
        let response = poll(
            deps.as_mut(),
            env.clone(),
            PollRequest {
                task_id: 42,
                wait_sequence: sequence,
                kind: "bridge-parent".into(),
                version: 1,
                step,
            },
            &registry,
            100,
        )
        .unwrap();
        assert!(response.messages.is_empty());
        let encoded: PollResponse = from_json(response.data.unwrap()).unwrap();
        match sequence {
            0 | 2 => {
                assert_eq!(encoded.messages.len(), 1);
                match &encoded.messages[0] {
                    CosmosMsg::Wasm(WasmMsg::Execute { msg, .. }) => {
                        assert_eq!(from_json::<(u64, u64)>(msg).unwrap(), (42, sequence + 1))
                    }
                    other => panic!("unexpected {other:?}"),
                }
                let PollOutcome::Pending { state, wait } = encoded.outcome else {
                    panic!("expected pending operation")
                };
                assert_eq!(
                    wait,
                    Wait::Operation {
                        resolver: env.contract.address.to_string(),
                        deadline: env.block.height + 10,
                        max_result_bytes: 100
                    }
                );
                step = PollStep::Resume {
                    state,
                    resolution: Resolution::Success {
                        data: to_json_binary(&7u32).unwrap(),
                    },
                };
            }
            1 => {
                assert!(encoded.messages.is_empty());
                let PollOutcome::Pending { state, wait } = encoded.outcome else {
                    panic!("expected child timer")
                };
                assert_eq!(
                    wait,
                    Wait::Height {
                        height: env.block.height + 2
                    }
                );
                env.block.height += 2;
                step = PollStep::Resume {
                    state,
                    resolution: Resolution::Timer {},
                };
            }
            3 => {
                assert!(encoded.messages.is_empty());
                let PollOutcome::Complete { output } = encoded.outcome else {
                    panic!("expected completion")
                };
                assert_eq!(from_json::<u32>(output).unwrap(), 14);
                return;
            }
            _ => unreachable!(),
        }
    }
}
