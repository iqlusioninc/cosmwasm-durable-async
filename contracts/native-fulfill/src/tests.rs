use super::*;
use cosmwasm_std::Storage;
use cosmwasm_std::{
    from_json,
    testing::{message_info, mock_dependencies, mock_env},
    to_json_binary, Addr, Binary,
};
use native_async::{PollOutcome, PollRequest, PollResponse, PollStep, Resolution, TaskSudoMsg};

#[test]
fn management_routes_forward_actual_sender_without_accepting_coins() {
    let mut deps = mock_dependencies();
    let sender = deps.api.addr_make("controller");
    let service = deps.api.addr_make("service").to_string();
    instantiate(
        deps.as_mut(),
        mock_env(),
        message_info(&sender, &[]),
        InstantiateMsg {
            payment_service: service.clone(),
            shipment_service: service,
            deadline_blocks: 10,
            fee_denom: "ugas".into(),
        },
    )
    .unwrap();
    for (json, expected) in [
        (
            r#"{"ExtendLease":{"task_id":7,"allocated_bytes":8192,"expires_at":1000}}"#,
            RuntimeMsg::ExtendLease {
                task_id: 7,
                actor: sender.to_string(),
                allocated_bytes: 8192,
                expires_at: 1000,
            },
        ),
        (
            r#"{"UpdateBudget":{"task_id":7,"gas_per_poll":100,"max_gas_price":"1","refund":"refund"}}"#,
            RuntimeMsg::UpdateBudget {
                task_id: 7,
                actor: sender.to_string(),
                gas_per_poll: 100,
                max_gas_price: cosmwasm_std::Decimal::one(),
                refund: "refund".into(),
            },
        ),
    ] {
        let msg: ExecuteMsg = from_json(json.as_bytes()).unwrap();
        let response = execute(
            native_deps(&mut deps),
            mock_env(),
            message_info(&sender, &[]),
            msg.clone(),
        )
        .unwrap();
        assert_eq!(
            response.messages[0].msg,
            cosmwasm_std::CosmosMsg::Custom(expected)
        );
        assert!(execute(
            native_deps(&mut deps),
            mock_env(),
            message_info(&sender, &cosmwasm_std::coins(1, "ugas")),
            msg
        )
        .is_err());
    }
}

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
        allocated_bytes: 524288,
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

fn runtime_deps(
    status: Option<native_async::TaskStatus>,
) -> cosmwasm_std::OwnedDeps<
    cosmwasm_std::testing::MockStorage,
    cosmwasm_std::testing::MockApi,
    cosmwasm_std::testing::MockQuerier<RuntimeQuery>,
    RuntimeQuery,
> {
    let querier = cosmwasm_std::testing::MockQuerier::<RuntimeQuery>::new(&[]).with_custom_handler(
        move |query| match (query, &status) {
            (
                RuntimeQuery::Funding(SchedulerQuery::TaskFunding { contract, task_id }),
                Some(status),
            ) => cosmwasm_std::SystemResult::Ok(cosmwasm_std::ContractResult::Ok(
                to_json_binary(&native_async::TaskFunding {
                    task_id: *task_id,
                    contract: contract.clone(),
                    control: String::new(),
                    refund: String::new(),
                    available: cosmwasm_std::Uint128::zero(),
                    reserved: cosmwasm_std::Uint128::zero(),
                    cumulative_execution_fees: cosmwasm_std::Uint128::zero(),
                    allocated_bytes: 0,
                    storage_lease_paid: cosmwasm_std::Uint128::zero(),
                    gas_per_poll: 0,
                    max_gas_price: cosmwasm_std::Decimal::one(),
                    max_attempt_charge: cosmwasm_std::Uint128::zero(),
                    expires_at: 0,
                    status: status.clone(),
                })
                .unwrap(),
            )),
            _ => cosmwasm_std::SystemResult::Ok(cosmwasm_std::ContractResult::Err(
                "task not found".into(),
            )),
        },
    );
    cosmwasm_std::OwnedDeps {
        storage: cosmwasm_std::testing::MockStorage::default(),
        api: cosmwasm_std::testing::MockApi::default(),
        querier,
        custom_query_type: std::marker::PhantomData,
    }
}

fn start_msg(business: u128, allocated_bytes: u64) -> ExecuteMsg {
    ExecuteMsg::Start {
        order: Order { id: 1 },
        business: cosmwasm_std::coin(business, "uatom"),
        gas: cosmwasm_std::coin(60, "ugas"),
        gas_per_poll: 100,
        max_gas_price: cosmwasm_std::Decimal::one(),
        expires_at: 20_000,
        allocated_bytes,
    }
}

fn instantiate_owner(
    deps: &mut cosmwasm_std::OwnedDeps<
        cosmwasm_std::testing::MockStorage,
        cosmwasm_std::testing::MockApi,
        cosmwasm_std::testing::MockQuerier<RuntimeQuery>,
        RuntimeQuery,
    >,
) -> Addr {
    let owner = deps.api.addr_make("owner");
    let service = deps.api.addr_make("service").to_string();
    let mut storage = std::mem::take(&mut deps.storage);
    let mut plain = mock_dependencies();
    std::mem::swap(&mut plain.storage, &mut storage);
    instantiate(
        plain.as_mut(),
        mock_env(),
        message_info(&owner, &[]),
        InstantiateMsg {
            payment_service: service.clone(),
            shipment_service: service,
            deadline_blocks: 10,
            fee_denom: "ugas".into(),
        },
    )
    .unwrap();
    deps.storage = plain.storage;
    owner
}

#[allow(deprecated)]
fn register_reply(payload: Binary, task_id: u64) -> cosmwasm_std::Reply {
    cosmwasm_std::Reply {
        id: REGISTER_REPLY,
        payload,
        gas_used: 1,
        result: cosmwasm_std::SubMsgResult::Ok(cosmwasm_std::SubMsgResponse {
            events: vec![],
            data: Some(to_json_binary(&task_id).unwrap()),
            msg_responses: vec![],
        }),
    }
}

#[test]
fn start_rejects_allocation_below_operation_result_reserve() {
    let mut deps = runtime_deps(None);
    let owner = instantiate_owner(&mut deps);
    let funds = [cosmwasm_std::coin(60, "ugas")];
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&owner, &funds),
        start_msg(0, 100_000),
    )
    .unwrap_err();
    assert!(err.to_string().contains("allocated_bytes"), "{err}");
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&owner, &funds),
        start_msg(0, 524_288),
    )
    .unwrap();
}

#[test]
fn business_funds_are_reclaimable_only_after_task_is_terminal() {
    for (status, height, reclaimable) in [
        (Some(native_async::TaskStatus::Runnable), 100, false),
        (Some(native_async::TaskStatus::Waiting), 100, false),
        (Some(native_async::TaskStatus::Completed), 100, true),
        (Some(native_async::TaskStatus::Cancelled), 100, true),
        (Some(native_async::TaskStatus::Failed), 100, true),
        // Expired records can be pruned; the lease height alone proves termination.
        (None, 20_000, true),
    ] {
        let mut deps = runtime_deps(status.clone());
        let owner = instantiate_owner(&mut deps);
        let funds = [
            cosmwasm_std::coin(40, "uatom"),
            cosmwasm_std::coin(60, "ugas"),
        ];
        let started = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner, &funds),
            start_msg(40, 524_288),
        )
        .unwrap();
        let payload = started.messages[0].payload.clone();
        reply(deps.as_mut(), mock_env(), register_reply(payload, 7)).unwrap();
        let mut env = mock_env();
        env.block.height = height;
        let stranger = deps.api.addr_make("stranger");
        assert!(execute(
            deps.as_mut(),
            env.clone(),
            message_info(&stranger, &[]),
            ExecuteMsg::Reclaim { task_id: 7 },
        )
        .is_err());
        let result = execute(
            deps.as_mut(),
            env.clone(),
            message_info(&owner, &[]),
            ExecuteMsg::Reclaim { task_id: 7 },
        );
        if !reclaimable {
            assert!(result.is_err(), "{status:?}");
            continue;
        }
        let response = result.unwrap();
        assert_eq!(
            response.messages[0].msg,
            CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
                to_address: owner.to_string(),
                amount: vec![cosmwasm_std::coin(40, "uatom")],
            })
        );
        assert!(execute(
            deps.as_mut(),
            env,
            message_info(&owner, &[]),
            ExecuteMsg::Reclaim { task_id: 7 },
        )
        .is_err());
    }
}
