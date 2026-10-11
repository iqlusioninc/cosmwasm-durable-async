use super::*;
use cosmwasm_std::Storage;
use cosmwasm_std::{
    from_json,
    testing::{message_info, mock_dependencies, mock_env},
    to_json_binary, Addr, Binary,
};
use native_async::{PollOutcome, PollRequest, PollResponse, PollStep, Resolution, TaskSudoMsg};

struct InvalidTimer;
impl Operation for InvalidTimer {
    const KIND: &'static str = durable_runtime::native::SLEEP_BLOCKS_KIND;
    type Request = bool;
    type Output = ();
    fn prepare(
        ctx: &mut WorkflowCtx<'_>,
        _: Correlation,
        time: bool,
    ) -> RuntimeResult<PreparedWait> {
        Ok(PreparedWait {
            operation: Self::KIND.into(),
            request: to_json_binary(&3u64)?,
            resolver: ctx.env().contract.address.clone(),
            deadline: if time {
                Deadline::Time(ctx.env().block.time.seconds() + 10)
            } else {
                Deadline::Height(ctx.env().block.height + 3)
            },
            messages: vec![cosmwasm_std::BankMsg::Send {
                to_address: ctx.env().contract.address.to_string(),
                amount: vec![],
            }
            .into()],
        })
    }
}
#[durable_workflow(kind = "invalid_timer", version = 1)]
async fn invalid_timer(ctx: WorkflowCtx, time: bool) -> Result<(), WaitError> {
    let done: () = ctx.wait::<InvalidTimer>(time).checkpoint().await?;
    Ok(done)
}
#[test]
fn bridge_rejects_time_deadlines_and_messages_on_reserved_timers() {
    let mut deps = mock_dependencies();
    let registry = Registry::new(vec![registration::<invalid_timer::Workflow>()]).unwrap();
    for time in [false, true] {
        let result = durable_runtime::native::poll(
            deps.as_mut(),
            mock_env(),
            PollRequest {
                task_id: 1,
                wait_sequence: 0,
                kind: "invalid_timer".into(),
                version: 1,
                step: PollStep::Start {
                    input: to_json_binary(&invalid_timer::Input { time }).unwrap(),
                },
            },
            &registry,
            100,
        );
        assert!(result.is_err());
    }
}

#[durable_workflow(kind = "timer_test", version = 1)]
async fn timer_test(ctx: WorkflowCtx, blocks: u64) -> Result<u64, WaitError> {
    let _done: () = ctx
        .wait::<durable_runtime::native::SleepBlocks>(blocks)
        .checkpoint(blocks)
        .await?;
    Ok(blocks)
}

#[test]
fn generated_height_timer_sleeps_and_resumes_with_unit_result() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let registry = Registry::new(vec![registration::<timer_test::Workflow>()]).unwrap();
    let request = |step, wait_sequence| PollRequest {
        task_id: 73,
        wait_sequence,
        kind: "timer_test".into(),
        version: 1,
        step,
    };
    let first = durable_runtime::native::poll(
        deps.as_mut(),
        env.clone(),
        request(
            PollStep::Start {
                input: to_json_binary(&timer_test::Input { blocks: 3 }).unwrap(),
            },
            0,
        ),
        &registry,
        65536,
    )
    .unwrap();
    let first: PollResponse = from_json(first.data.unwrap()).unwrap();
    let PollOutcome::Pending { state, wait } = first.outcome else {
        panic!("pending")
    };
    assert_eq!(
        wait,
        native_async::Wait::Height {
            height: env.block.height + 3
        }
    );
    assert!(first.messages.is_empty());
    let last = durable_runtime::native::poll(
        deps.as_mut(),
        env,
        request(
            PollStep::Resume {
                state,
                resolution: Resolution::Timer {},
            },
            1,
        ),
        &registry,
        65536,
    )
    .unwrap();
    let last: PollResponse = from_json(last.data.unwrap()).unwrap();
    assert!(
        matches!(last.outcome, PollOutcome::Complete { output } if from_json::<u64>(&output).unwrap() == 3)
    );
}

#[test]
fn bridge_rejects_unknown_identity_malformed_state_and_wrong_wakeup() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let registry = Registry::new(vec![registration::<timer_test::Workflow>()]).unwrap();
    for request in [
        PollRequest {
            task_id: 1,
            wait_sequence: 0,
            kind: "unknown".into(),
            version: 1,
            step: PollStep::Start {
                input: Binary::default(),
            },
        },
        PollRequest {
            task_id: 1,
            wait_sequence: 0,
            kind: "timer_test".into(),
            version: 2,
            step: PollStep::Start {
                input: Binary::default(),
            },
        },
        PollRequest {
            task_id: 1,
            wait_sequence: 1,
            kind: "timer_test".into(),
            version: 1,
            step: PollStep::Resume {
                state: Binary::from(b"{}"),
                resolution: Resolution::Timer {},
            },
        },
    ] {
        assert!(
            durable_runtime::native::poll(deps.as_mut(), env.clone(), request, &registry, 100)
                .is_err()
        );
    }
    let first = durable_runtime::native::poll(
        deps.as_mut(),
        env.clone(),
        PollRequest {
            task_id: 1,
            wait_sequence: 0,
            kind: "timer_test".into(),
            version: 1,
            step: PollStep::Start {
                input: to_json_binary(&timer_test::Input { blocks: 3 }).unwrap(),
            },
        },
        &registry,
        100,
    )
    .unwrap();
    let first: PollResponse = from_json(first.data.unwrap()).unwrap();
    let PollOutcome::Pending { state, .. } = first.outcome else {
        panic!("pending")
    };
    assert!(durable_runtime::native::poll(
        deps.as_mut(),
        env,
        PollRequest {
            task_id: 1,
            wait_sequence: 1,
            kind: "timer_test".into(),
            version: 1,
            step: PollStep::Resume {
                state,
                resolution: Resolution::Success {
                    data: to_json_binary(&()).unwrap()
                }
            },
        },
        &registry,
        100
    )
    .is_err());
}

#[test]
fn start_splits_same_denom_and_rejects_double_counting() {
    let mut deps = mock_dependencies();
    let payment_service = deps.api.addr_make("payment").to_string();
    let shipment_service = deps.api.addr_make("shipment").to_string();
    let owner = deps.api.addr_make("owner");
    instantiate(
        deps.as_mut(),
        mock_env(),
        message_info(&owner, &[]),
        InstantiateMsg {
            payment_service,
            shipment_service,
            deadline_blocks: 10,
            fee_denom: "ugas".into(),
        },
    )
    .unwrap();
    let start = || ExecuteMsg::Start {
        order: Order { id: 1 },
        business: cosmwasm_std::coin(40, "ugas"),
        gas: cosmwasm_std::coin(60, "ugas"),
        gas_per_poll: 100,
        max_gas_price: cosmwasm_std::Decimal::one(),
        expires_at: 999999,
        allocated_bytes: 131072,
    };
    assert!(execute(
        native_deps(&mut deps),
        mock_env(),
        message_info(&owner, &[cosmwasm_std::coin(60, "ugas")]),
        start()
    )
    .is_err());
    let response = execute(
        native_deps(&mut deps),
        mock_env(),
        message_info(&owner, &[cosmwasm_std::coin(100, "ugas")]),
        start(),
    )
    .unwrap();
    assert_eq!(
        response.messages[0].reply_on,
        cosmwasm_std::ReplyOn::Success
    );
    assert_eq!(response.messages[0].id, REGISTER_REPLY);
    let CosmosMsg::Custom(RuntimeMsg::Register { funds, .. }) = &response.messages[0].msg else {
        panic!("register")
    };
    assert_eq!(*funds, cosmwasm_std::coin(60, "ugas"));
}

fn native_deps(
    deps: &mut cosmwasm_std::OwnedDeps<
        cosmwasm_std::testing::MockStorage,
        cosmwasm_std::testing::MockApi,
        cosmwasm_std::testing::MockQuerier,
    >,
) -> cosmwasm_std::DepsMut<'_, RuntimeQuery> {
    cosmwasm_std::DepsMut {
        storage: &mut deps.storage,
        api: &deps.api,
        querier: cosmwasm_std::QuerierWrapper::new(&deps.querier),
    }
}

#[test]
#[allow(deprecated)]
fn task_id_reply_is_host_id_and_requires_successful_payload() {
    let mut deps = mock_dependencies();
    let response = reply(
        native_deps(&mut deps),
        mock_env(),
        cosmwasm_std::Reply {
            id: REGISTER_REPLY,
            payload: Binary::default(),
            gas_used: 1,
            result: cosmwasm_std::SubMsgResult::Ok(cosmwasm_std::SubMsgResponse {
                events: vec![],
                data: Some(to_json_binary(&937u64).unwrap()),
                msg_responses: vec![],
            }),
        },
    )
    .unwrap();
    assert_eq!(from_json::<u64>(response.data.unwrap()).unwrap(), 937);
    assert!(reply(
        native_deps(&mut deps),
        mock_env(),
        cosmwasm_std::Reply {
            id: REGISTER_REPLY,
            payload: Binary::default(),
            gas_used: 1,
            result: cosmwasm_std::SubMsgResult::Ok(cosmwasm_std::SubMsgResponse {
                events: vec![],
                data: None,
                msg_responses: vec![]
            }),
        }
    )
    .is_err());
}

#[test]
fn callback_forwards_actual_sender_and_external_error() {
    let mut deps = mock_dependencies();
    let payment_service = deps.api.addr_make("payment").to_string();
    let shipment_service = deps.api.addr_make("shipment").to_string();
    instantiate(
        deps.as_mut(),
        mock_env(),
        message_info(&Addr::unchecked("owner"), &[]),
        InstantiateMsg {
            payment_service,
            shipment_service,
            deadline_blocks: 10,
            fee_denom: "ugas".into(),
        },
    )
    .unwrap();
    let sender = deps.api.addr_make("resolver");
    let response = execute(
        native_deps(&mut deps),
        mock_env(),
        message_info(&sender, &[]),
        ExecuteMsg::Resume {
            workflow_id: 42,
            wait_sequence: 2,
            outcome: durable_runtime::Outcome::Error {
                code: "declined".into(),
                message: "no".into(),
            },
        },
    )
    .unwrap();
    let CosmosMsg::Custom(RuntimeMsg::Deliver {
        task_id,
        resolver,
        resolution,
        ..
    }) = &response.messages[0].msg
    else {
        panic!("deliver")
    };
    assert_eq!(*task_id, 42);
    assert_eq!(resolver, sender.as_str());
    assert!(matches!(resolution, Resolution::Error { code, .. } if code == "declined"));
}

#[test]
fn application_failure_is_encoded_and_commits_prior_payment_write() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let payment_service = deps.api.addr_make("payment").to_string();
    let shipment_service = deps.api.addr_make("shipment").to_string();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&Addr::unchecked("owner"), &[]),
        InstantiateMsg {
            payment_service,
            shipment_service,
            deadline_blocks: 10,
            fee_denom: "ugas".into(),
        },
    )
    .unwrap();
    let make = |step, wait_sequence| {
        TaskSudoMsg::PollTask(PollRequest {
            task_id: 1,
            wait_sequence,
            kind: "fulfill".into(),
            version: 1,
            step,
        })
    };
    let first = sudo(
        deps.as_mut(),
        env.clone(),
        make(
            PollStep::Start {
                input: to_json_binary(&fulfill::Input {
                    order: Order { id: 7 },
                })
                .unwrap(),
            },
            0,
        ),
    )
    .unwrap();
    let first: PollResponse = from_json(first.data.unwrap()).unwrap();
    let PollOutcome::Pending { state, .. } = first.outcome else {
        panic!("pending")
    };
    let fail = sudo(
        deps.as_mut(),
        env,
        make(
            PollStep::Resume {
                state,
                resolution: Resolution::Success {
                    data: to_json_binary(&Payment {
                        reference: "".into(),
                    })
                    .unwrap(),
                },
            },
            1,
        ),
    )
    .unwrap();
    let fail: PollResponse = from_json(fail.data.unwrap()).unwrap();
    assert!(
        matches!(fail.outcome, PollOutcome::Fail { error } if error.contains("PaymentRejected"))
    );
    let saved: Payment = from_json(deps.storage.get(&payment_key(7)).unwrap()).unwrap();
    assert_eq!(saved.reference, "");
}

#[test]
fn generated_two_waits_use_fresh_contexts_and_next_sequence() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let payment_service = deps.api.addr_make("payment").to_string();
    let shipment_service = deps.api.addr_make("shipment").to_string();
    instantiate(
        deps.as_mut().into_empty(),
        env.clone(),
        message_info(&Addr::unchecked("owner"), &[]),
        InstantiateMsg {
            payment_service,
            shipment_service,
            deadline_blocks: 10,
            fee_denom: "ugas".into(),
        },
    )
    .unwrap();
    let poll = |step, wait_sequence| {
        TaskSudoMsg::PollTask(PollRequest {
            task_id: 37,
            wait_sequence,
            kind: "fulfill".into(),
            version: 1,
            step,
        })
    };
    let first = sudo(
        deps.as_mut().into_empty(),
        env.clone(),
        poll(
            PollStep::Start {
                input: to_json_binary(&fulfill::Input {
                    order: Order { id: 9 },
                })
                .unwrap(),
            },
            0,
        ),
    )
    .unwrap();
    assert!(first.messages.is_empty());
    let first: PollResponse = from_json(first.data.unwrap()).unwrap();
    let state = match first.outcome {
        PollOutcome::Pending { state, .. } => state,
        _ => panic!("expected wait"),
    };
    let cosmwasm_std::CosmosMsg::Wasm(cosmwasm_std::WasmMsg::Execute { msg, .. }) =
        &first.messages[0]
    else {
        panic!("request")
    };
    let ServiceMsg::Begin { correlation, .. } = from_json(msg).unwrap();
    assert_eq!(
        correlation,
        durable_runtime::Correlation {
            workflow_id: 37,
            wait_sequence: 1
        }
    );
    let malformed = sudo(
        deps.as_mut().into_empty(),
        env.clone(),
        poll(
            PollStep::Resume {
                state: state.clone(),
                resolution: Resolution::Success {
                    data: Binary::from(b"{}"),
                },
            },
            1,
        ),
    );
    assert!(malformed.is_err());
    assert!(deps.storage.get(&payment_key(9)).is_none());
    let second = sudo(
        deps.as_mut().into_empty(),
        env.clone(),
        poll(
            PollStep::Resume {
                state,
                resolution: Resolution::Success {
                    data: to_json_binary(&Payment {
                        reference: "paid".into(),
                    })
                    .unwrap(),
                },
            },
            1,
        ),
    )
    .unwrap();
    let second: PollResponse = from_json(second.data.unwrap()).unwrap();
    let state = match second.outcome {
        PollOutcome::Pending { state, .. } => state,
        _ => panic!("expected wait"),
    };
    let cosmwasm_std::CosmosMsg::Wasm(cosmwasm_std::WasmMsg::Execute { msg, .. }) =
        &second.messages[0]
    else {
        panic!("request")
    };
    let ServiceMsg::Begin { correlation, .. } = from_json(msg).unwrap();
    assert_eq!(correlation.wait_sequence, 2);
    let last = sudo(
        deps.as_mut().into_empty(),
        env,
        poll(
            PollStep::Resume {
                state,
                resolution: Resolution::Success {
                    data: to_json_binary(&Shipment {
                        tracking: "sent".into(),
                    })
                    .unwrap(),
                },
            },
            2,
        ),
    )
    .unwrap();
    let last: PollResponse = from_json(last.data.unwrap()).unwrap();
    let PollOutcome::Complete { output } = last.outcome else {
        panic!("complete")
    };
    assert_eq!(from_json::<Receipt>(output).unwrap().order_id, 9);
}
