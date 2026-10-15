mod support;
use cosmwasm_std::*;
use cw_multi_test::Executor;
use native_async::*;
use native_dca::msg::*;
use support::fixtures::Fault;
use support::DcaSuite;

fn business(s: &DcaSuite) -> Vec<Coin> {
    [
        &s.owner,
        &s.sponsor,
        &s.contract,
        &s.exchange,
        &s.beneficiary,
    ]
    .into_iter()
    .flat_map(|address| {
        ["uinput", "uoutput"].map(|denom| s.app.wrap().query_balance(address, denom).unwrap())
    })
    .collect()
}
fn manage(s: &mut DcaSuite, actor: Addr, msg: AsyncExecuteMsg, funds: &[Coin]) {
    s.app
        .execute_contract(actor, s.contract.clone(), &ExecuteMsg::Async(msg), funds)
        .unwrap();
}

#[test]
fn every_nested_fault_rolls_back_business_but_charges_execution() {
    // Each row removes one atomicity/authentication check or nested effect.
    for fault in [
        Fault::Omit,
        Fault::Duplicate,
        Fault::Stale,
        Fault::WrongOrder,
        Fault::WrongSlice,
        Fault::WrongTask,
        Fault::WrongSender,
        Fault::InputAmount,
        Fault::InputDenom,
        Fault::OutputDenom,
        Fault::Beneficiary,
        Fault::BelowMinimum,
        Fault::BankFailure,
        Fault::DeliveryFailure,
        Fault::ReplyFailure,
        Fault::ReplyCrossOrder,
        Fault::ReplyReplay,
        Fault::Pad2049,
    ] {
        let mut s = DcaSuite::with_faults();
        s.set_fault(fault);
        s.create(s.owner.clone(), s.request()).unwrap();
        s.block(2).unwrap();
        let before = (s.order(1).order, business(&s));
        let before_fees = s.funding(1).cumulative_execution_fees;
        let dca_storage: Vec<_> = s
            .app
            .contract_storage(&s.contract)
            .range(None, None, Order::Ascending)
            .collect();
        let exchange_storage: Vec<_> = s
            .app
            .contract_storage(&s.exchange)
            .range(None, None, Order::Ascending)
            .collect();
        let report = s.block(6).unwrap();
        assert_eq!(report.polls, 1, "{fault:?}");
        assert_eq!((s.order(1).order, business(&s)), before, "{fault:?}");
        assert_eq!(
            s.app
                .contract_storage(&s.contract)
                .range(None, None, Order::Ascending)
                .collect::<Vec<_>>(),
            dca_storage,
            "{fault:?}"
        );
        assert_eq!(
            s.app
                .contract_storage(&s.exchange)
                .range(None, None, Order::Ascending)
                .collect::<Vec<_>>(),
            exchange_storage,
            "{fault:?}"
        );
        assert!(
            s.fill(1, 0).is_none() && s.receipt(1, 0).is_none(),
            "{fault:?}"
        );
        assert!(
            matches!(
                s.task(1).status,
                TaskStatus::Paused {
                    reason: PauseReason::MessageFailure,
                    ..
                }
            ),
            "{fault:?}: {:?}",
            s.task(1)
        );
        assert_eq!(s.task(1).wait_sequence, 1);
        // The due timer promotion precedes the poll transaction; only its old
        // Timer resolution survives, never the failed callback's Success.
        assert_eq!(s.task(1).inbox, Some(Resolution::Timer {}));
        assert_eq!(report.gas_used, 14);
        assert_eq!(
            s.funding(1).cumulative_execution_fees - before_fees,
            Uint128::new(14)
        );
        assert_eq!(s.funding(1).available, Uint128::new(976));
        assert_eq!(s.block(6).unwrap().polls, 0);
        s.conservation(1);
    }
}

fn custom_deps() -> OwnedDeps<
    testing::MockStorage,
    testing::MockApi,
    testing::MockQuerier<RuntimeQuery>,
    RuntimeQuery,
> {
    OwnedDeps {
        storage: testing::MockStorage::new(),
        api: testing::MockApi::default(),
        querier: testing::MockQuerier::new(&[]),
        custom_query_type: std::marker::PhantomData,
    }
}

#[test]
fn dispatch_authenticates_authoritative_staged_identity_before_writing() {
    // Each alteration isolates a required staged identity check. Unit handler
    // calls establish validation only; the fault matrix proves transactions.
    use native_dca::state::*;
    let mut s = DcaSuite::new();
    s.create(s.owner.clone(), s.request()).unwrap();
    let order = s.order(1).order;
    s.block(2).unwrap();
    s.block(6).unwrap();
    let mut baseline = s.task(1);
    baseline.inbox = None;
    let mut env = testing::mock_env();
    env.block.height = 6;
    env.contract.address = s.contract.clone();
    let correlation = dca_example_protocol::Correlation {
        workflow_id: 1,
        wait_sequence: 2,
    };
    for status in [TaskStatus::Runnable, TaskStatus::Waiting] {
        let mut task = baseline.clone();
        task.status = status;
        let mut deps = custom_deps();
        save_order(&mut deps.storage, &order).unwrap();
        deps.querier = deps.querier.with_custom_handler(move |_| {
            SystemResult::Ok(ContractResult::Ok(to_json_binary(&task).unwrap()))
        });
        let response = native_dca::settlement::dispatch_slice(
            deps.as_mut(),
            env.clone(),
            MessageInfo {
                sender: s.contract.clone(),
                funds: vec![],
            },
            1,
            0,
            correlation,
        )
        .unwrap();
        assert_eq!(response.messages.len(), 1);
        assert_eq!(response.messages[0].id, 2);
        let CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr,
            funds,
            ..
        }) = &response.messages[0].msg
        else {
            panic!("missing exchange call");
        };
        assert_eq!(contract_addr, s.exchange.as_str());
        assert_eq!(funds, &[coin(10, "uinput")]);
        assert_eq!(
            load_order(&deps.storage, 1).unwrap().remaining,
            Uint128::new(20)
        );
    }
    for mutation in 0..18 {
        let mut task = baseline.clone();
        match mutation {
            0 => task.contract = s.owner.to_string(), 1 => task.task_id = 2,
            2 => task.program.kind = "other".into(), 3 => task.program.version = 2,
            4 => task.program.input = to_json_binary(&WorkflowInput { order_id: 1 }).unwrap(),
            5 => task.status = TaskStatus::Cancelled, 6 => task.wait_sequence = 3,
            7 => task.expires_at = 101, 8 => task.last_attempt = Some(5),
            9 => task.inbox = Some(Resolution::Success { data: Binary::default() }),
            10 => task.wait = Some(Wait::Operation { resolver: s.owner.to_string(), deadline: 7, max_result_bytes: 2048 }),
            11 => task.wait = Some(Wait::Operation { resolver: s.exchange.to_string(), deadline: 8, max_result_bytes: 2048 }),
            12 => task.wait = Some(Wait::Operation { resolver: s.exchange.to_string(), deadline: 7, max_result_bytes: 2049 }),
            13 => task.state = Some(Binary::from(br#"{"Waiting2":{"order_id":2,"current":{"slice_index":0,"sleep_blocks":0,"state":"Ready"}}}"#.as_slice())),
            14 => task.state = Some(Binary::from(br#"{"Waiting2":{"order_id":1,"current":{"slice_index":1,"sleep_blocks":0,"state":"Ready"}}}"#.as_slice())),
            15 => task.state = Some(Binary::from(br#"{"Waiting1":{"order_id":1,"current":{"slice_index":0,"sleep_blocks":0,"state":"Ready"}}}"#.as_slice())),
            16 => task.output = Some(Binary::default()), 17 => task.error = Some("unexpected".into()), _ => unreachable!(),
        }
        let mut deps = custom_deps();
        save_order(&mut deps.storage, &order).unwrap();
        deps.querier = deps.querier.with_custom_handler(move |_| {
            SystemResult::Ok(ContractResult::Ok(to_json_binary(&task).unwrap()))
        });
        let before: Vec<_> = deps.storage.range(None, None, Order::Ascending).collect();
        assert!(
            native_dca::settlement::dispatch_slice(
                deps.as_mut(),
                env.clone(),
                MessageInfo {
                    sender: s.contract.clone(),
                    funds: vec![]
                },
                1,
                0,
                correlation
            )
            .is_err(),
            "mutation {mutation}"
        );
        assert_eq!(
            deps.storage
                .range(None, None, Order::Ascending)
                .collect::<Vec<_>>(),
            before
        );
    }
    let mut deps = custom_deps();
    save_order(&mut deps.storage, &order).unwrap();
    for info in [
        MessageInfo {
            sender: s.owner.clone(),
            funds: vec![],
        },
        MessageInfo {
            sender: s.contract.clone(),
            funds: vec![coin(1, "uinput")],
        },
    ] {
        assert!(native_dca::settlement::dispatch_slice(
            deps.as_mut(),
            env.clone(),
            info,
            1,
            0,
            correlation
        )
        .is_err());
        assert_eq!(load_order(&deps.storage, 1).unwrap(), order);
    }
}

#[test]
fn canonical_callback_encoder_checks_exact_escaped_byte_boundary() {
    // Synthetic stored beneficiary deliberately bypasses admission address
    // validation to isolate canonical receipt byte enforcement. This is not a
    // claim that a 2048-byte beneficiary is an admitted address.
    use dca_example_protocol::{Correlation, SwapReceipt, SwapRequest};
    use native_dca::state::*;
    let mut s = DcaSuite::new();
    s.create(s.owner.clone(), s.request()).unwrap();
    for size in [2048, 2049] {
        let mut order = s.order(1).order;
        order.remaining = Uint128::new(20);
        let mut receipt = SwapReceipt {
            order_id: 1,
            slice_index: 0,
            correlation: Correlation {
                workflow_id: 1,
                wait_sequence: 2,
            },
            input: coin(10, "uinput"),
            output: coin(15, "uoutput"),
            beneficiary: "\"\\".repeat(400),
        };
        let base = to_json_binary(&receipt).unwrap().len();
        receipt.beneficiary.push_str(&"x".repeat(size - base));
        assert_eq!(to_json_binary(&receipt).unwrap().len(), size);
        order.beneficiary = Addr::unchecked(&receipt.beneficiary);
        let pending = PendingDispatch {
            request: SwapRequest {
                order_id: 1,
                slice_index: 0,
                correlation: receipt.correlation,
                input: receipt.input.clone(),
                output_denom: "uoutput".into(),
                minimum_output: Uint128::new(14),
                beneficiary: receipt.beneficiary.clone(),
            },
            dispatch_height: 6,
            deadline: 7,
        };
        order.pending = Some(pending.clone());
        let mut deps = custom_deps();
        save_order(&mut deps.storage, &order).unwrap();
        deps.storage.set(
            b"dca/v1/dispatch-reply/00000000000000000001",
            &to_json_vec(&pending).unwrap(),
        );
        let mut env = testing::mock_env();
        env.block.height = 6;
        let result = native_dca::settlement::swap_result(
            deps.as_mut(),
            env,
            MessageInfo {
                sender: s.exchange.clone(),
                funds: vec![],
            },
            receipt,
        );
        if size == 2048 {
            let response = result.unwrap();
            let CosmosMsg::Custom(RuntimeMsg::Deliver {
                resolver,
                resolution: Resolution::Success { data },
                ..
            }) = &response.messages[0].msg
            else {
                panic!("missing actual-sender delivery");
            };
            assert_eq!(data.len(), 2048);
            assert_eq!(resolver, s.exchange.as_str());
            assert_eq!(load_order(&deps.storage, 1).unwrap().filled_count, 1);
        } else {
            assert!(result.is_err());
            assert_eq!(load_order(&deps.storage, 1).unwrap(), order);
            assert!(load_fill(&deps.storage, 1, 0).unwrap().is_none());
        }
    }
}

#[test]
fn exact_result_limit_is_enforced_by_real_host_delivery() {
    // Catches a >= instead of > host bound; 2049 rollback is in the fault table.
    let mut s = DcaSuite::with_faults();
    s.set_fault(Fault::Pad2048);
    s.create(s.owner.clone(), s.request()).unwrap();
    s.block(2).unwrap();
    s.block(6).unwrap();
    let Resolution::Success { data } = s.task(1).inbox.unwrap() else {
        panic!("missing committed success");
    };
    assert_eq!(data.len(), 2048);
    use durable_runtime::{Operation, Outcome};
    let decoded = native_dca::workflow::SwapSlice::decode(Outcome::Success(data.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(
        native_dca::workflow::SwapSlice::decode(Outcome::Success(data))
            .unwrap()
            .unwrap(),
        decoded
    );
    assert_eq!(decoded, s.fill(1, 0).unwrap().receipt);
    assert_eq!(s.order(1).order.spent, Uint128::new(10));
    s.block(7).unwrap();
    assert_eq!(s.task(1).wait, Some(Wait::Height { height: 11 }));
    s.conservation(1);
}

#[test]
fn insufficient_liquidity_requires_explicit_resume_after_recovery() {
    let mut s = DcaSuite::new();
    s.app
        .send_tokens(
            s.exchange.clone(),
            s.owner.clone(),
            &[coin(100_000, "uoutput")],
        )
        .unwrap();
    s.create(s.owner.clone(), s.request()).unwrap();
    s.block(2).unwrap();
    s.block(6).unwrap();
    assert_eq!(s.order(1).order.filled_count, 0);
    assert!(s.receipt(1, 0).is_none());
    assert_eq!(s.funding(1).cumulative_execution_fees, Uint128::new(23));
    s.app
        .send_tokens(s.owner.clone(), s.exchange.clone(), &[coin(100, "uoutput")])
        .unwrap();
    assert_eq!(s.block(7).unwrap().polls, 0);
    manage(
        &mut s,
        s_owner(),
        AsyncExecuteMsg::ResumeTask { task_id: 1 },
        &[],
    );
    s.block(8).unwrap();
    assert_eq!(
        (
            s.order(1).order.filled_count,
            s.order(1).order.last_fill_height
        ),
        (1, Some(8))
    );
    s.block(9).unwrap();
    assert_eq!(s.task(1).wait, Some(Wait::Height { height: 13 }));
    s.conservation(1);
}
fn s_owner() -> Addr {
    cosmwasm_std::testing::MockApi::default().addr_make("dca-owner")
}

#[test]
fn sponsor_topup_is_not_control_or_implicit_resume() {
    let mut s = DcaSuite::new();
    let mut request = s.request();
    request.gas = coin(101, "ugas");
    s.create(s.owner.clone(), request).unwrap();
    s.block(2).unwrap();
    s.block(6).unwrap();
    assert!(matches!(
        s.task(1).status,
        TaskStatus::Paused {
            reason: PauseReason::InsufficientFunds,
            ..
        }
    ));
    let sponsor = s.sponsor.clone();
    manage(
        &mut s,
        sponsor.clone(),
        AsyncExecuteMsg::FundTask { task_id: 1 },
        &[coin(500, "ugas")],
    );
    assert_eq!(s.block(7).unwrap().polls, 0);
    assert!(s
        .app
        .execute_contract(
            sponsor,
            s.contract.clone(),
            &ExecuteMsg::Async(AsyncExecuteMsg::ResumeTask { task_id: 1 }),
            &[]
        )
        .is_err());
    assert_eq!(s.order(1).order.filled_count, 0);
    assert_eq!(s.funding(1).control, s.owner.to_string());
    assert_eq!(s.funding(1).refund, s.owner.to_string());
    manage(
        &mut s,
        s_owner(),
        AsyncExecuteMsg::ResumeTask { task_id: 1 },
        &[],
    );
    s.block(8).unwrap();
    assert_eq!(s.order(1).order.filled_count, 1);
    s.conservation(1);
}

#[test]
fn price_pause_and_budget_change_do_not_implicitly_resume() {
    let mut s = DcaSuite::new();
    s.create(s.owner.clone(), s.request()).unwrap();
    s.app.init_modules(|router, _, _| {
        router.custom.runtime.keeper.config.gas_price = Decimal::from_ratio(2u128, 1u128);
        router.custom.runtime.keeper.config.min_base_price = Decimal::from_ratio(2u128, 1u128);
    });
    s.block(5).unwrap();
    assert!(matches!(
        s.task(1).status,
        TaskStatus::Paused {
            reason: PauseReason::PriceLimit,
            ..
        }
    ));
    let update = ExecuteMsg::UpdateBudget {
        order_id: 1,
        gas_per_poll: 200,
        max_gas_price: Decimal::from_ratio(2u128, 1u128),
    };
    assert!(s
        .app
        .execute_contract(s.sponsor.clone(), s.contract.clone(), &update, &[])
        .is_err());
    s.app
        .execute_contract(s.owner.clone(), s.contract.clone(), &update, &[])
        .unwrap();
    assert_eq!(s.block(6).unwrap().polls, 0);
    manage(
        &mut s,
        s_owner(),
        AsyncExecuteMsg::ResumeTask { task_id: 1 },
        &[],
    );
    s.block(7).unwrap();
    assert_eq!(s.order(1).order.filled_count, 1);
    assert_eq!(s.funding(1).gas_per_poll, 200);
    s.conservation(1);
}

#[test]
fn reconstruct_before_timer_and_between_receipt_and_consumption() {
    // Catches lost continuation, bank state, fixture identity or duplicate effects.
    let mut original = DcaSuite::new();
    let mut rebuilt = DcaSuite::new();
    original
        .create(original.owner.clone(), original.request())
        .unwrap();
    rebuilt
        .create(rebuilt.owner.clone(), rebuilt.request())
        .unwrap();
    original.block(2).unwrap();
    rebuilt.block(2).unwrap();
    rebuilt.reconstruct().unwrap();
    for height in [6, 7, 11, 12, 16, 17] {
        original.block(height).unwrap();
        rebuilt.block(height).unwrap();
        assert_eq!(rebuilt.task(1), original.task(1));
        assert_eq!(rebuilt.order(1), original.order(1));
        assert_eq!(rebuilt.funding(1), original.funding(1));
        assert_eq!(business(&rebuilt), business(&original));
        rebuilt.conservation(1);
        original.conservation(1);
        if height == 6 {
            assert!(rebuilt.task(1).inbox.is_some());
            rebuilt.reconstruct().unwrap();
        }
    }
    assert_eq!(rebuilt.task(1).status, TaskStatus::Completed);
    assert_eq!(
        rebuilt
            .app
            .wrap()
            .query_balance(&rebuilt.beneficiary, "uoutput")
            .unwrap()
            .amount,
        Uint128::new(45)
    );
}

#[test]
fn terminal_diagnostics_never_retry_or_erase_committed_fills() {
    for timeout in [false, true] {
        let mut s = DcaSuite::with_faults();
        if !timeout {
            s.set_fault(Fault::RemoteError);
        }
        s.create(s.owner.clone(), s.request()).unwrap();
        s.block(2).unwrap();
        s.block(6).unwrap();
        assert_eq!(s.order(1).order.filled_count, 1);
        if timeout {
            // Explicit persisted-host fault injection: atomic production replies
            // cannot legitimately leave an operation without its success inbox.
            let key = [
                b"na2/task/".as_slice(),
                s.contract.as_bytes(),
                b"/",
                &1u64.to_be_bytes(),
            ]
            .concat();
            let mut task: native_async::runtime::ExecutionTask =
                from_json(s.app.storage().get(&key).unwrap()).unwrap();
            task.inbox = None;
            s.app.storage_mut().set(&key, &to_json_vec(&task).unwrap());
        }
        s.block(7).unwrap();
        assert_eq!(s.task(1).status, TaskStatus::Failed);
        assert_eq!(
            s.task(1).error.as_deref(),
            Some(if timeout { "\"Timeout\"" } else { "\"Remote\"" })
        );
        assert_eq!(
            s.task(1).allocated_bytes,
            native_dca::contract::MIN_ALLOCATED_BYTES
        );
        s.block(50).unwrap();
        assert_eq!(s.order(1).order.filled_count, 1);
        assert_eq!(
            s.app
                .wrap()
                .query_balance(&s.beneficiary, "uoutput")
                .unwrap()
                .amount,
            Uint128::new(15)
        );
        s.app
            .execute_contract(
                s.owner.clone(),
                s.contract.clone(),
                &ExecuteMsg::ReclaimPrincipal { order_id: 1 },
                &[],
            )
            .unwrap();
        assert_eq!(s.order(1).order.reclaimed, Uint128::new(20));
        s.conservation(1);
    }
}

#[test]
fn cancel_before_fill_and_expire_after_receipt_allow_exact_reclaim() {
    for after_receipt in [false, true] {
        let mut s = DcaSuite::new();
        let mut request = s.request();
        request.expires_at = 8;
        s.create(s.owner.clone(), request).unwrap();
        s.block(2).unwrap();
        if after_receipt {
            s.block(6).unwrap();
            assert!(s.task(1).inbox.is_some());
            s.block(8).unwrap();
            assert_eq!(s.task(1).status, TaskStatus::Expired);
        } else {
            manage(
                &mut s,
                s_owner(),
                AsyncExecuteMsg::CancelTask { task_id: 1 },
                &[],
            );
        }
        s.app
            .execute_contract(
                s.owner.clone(),
                s.contract.clone(),
                &ExecuteMsg::ReclaimPrincipal { order_id: 1 },
                &[],
            )
            .unwrap();
        assert_eq!(
            s.order(1).order.reclaimed,
            Uint128::new(if after_receipt { 20 } else { 30 })
        );
        assert_eq!(s.order(1).order.filled_count, u32::from(after_receipt));
        s.conservation(1);
        assert!(s
            .app
            .execute_contract(
                s.owner.clone(),
                s.contract.clone(),
                &ExecuteMsg::ReclaimPrincipal { order_id: 1 },
                &[]
            )
            .is_err());
    }
    let mut s = DcaSuite::new();
    s.create(s.owner.clone(), s.request()).unwrap();
    s.block(2).unwrap();
    s.block(6).unwrap();
    s.block(111).unwrap();
    assert!(s
        .app
        .wrap()
        .query_wasm_smart::<TaskResponse>(&s.contract, &QueryMsg::Task { order_id: 1 })
        .is_err());
    s.app
        .execute_contract(
            s.owner.clone(),
            s.contract.clone(),
            &ExecuteMsg::ReclaimPrincipal { order_id: 1 },
            &[],
        )
        .unwrap();
    assert_eq!(s.order(1).order.reclaimed, Uint128::new(20));
    s.conservation(1);
}

#[test]
fn two_owners_and_two_contract_addresses_never_alias_ids_or_fills() {
    let mut s = DcaSuite::new();
    s.app.contract_storage_mut(&s.contract).set(
        native_dca::state::NEXT_ORDER_ID_KEY,
        &to_json_vec(&7u64).unwrap(),
    );
    let mut request = s.request();
    request.slice_count = 1;
    let first = s.create(s.owner.clone(), request.clone()).unwrap();
    request.beneficiary = s.sponsor.to_string();
    let second = s.create(s.sponsor.clone(), request).unwrap();
    assert_eq!(
        (
            first.order_id,
            first.task_id,
            second.order_id,
            second.task_id
        ),
        (7, 1, 8, 2)
    );
    let code = s.app.store_code(support::dca_contract());
    let other = s
        .app
        .instantiate_contract(
            code,
            s.sponsor.clone(),
            &InstantiateMsg {
                exchange: s.exchange.to_string(),
                fee_denom: "ugas".into(),
            },
            &[],
            "other-dca",
            None,
        )
        .unwrap();
    s.app.init_modules(|router, _, _| {
        router.custom.identities.insert(
            other.to_string(),
            Binary::from(b"immutable-rust-native-dca-v1".as_slice()),
        );
    });
    let original = std::mem::replace(&mut s.contract, other.clone());
    let mut request = s.request();
    request.slice_count = 1;
    let third = s.create(s.sponsor.clone(), request).unwrap();
    assert_eq!((third.order_id, third.task_id), (1, 1));
    s.contract = original.clone();
    s.block(2).unwrap();
    s.block(6).unwrap();
    assert_eq!(s.fill(7, 0).unwrap().receipt.correlation.wait_sequence, 2);
    assert_eq!(s.fill(8, 0).unwrap().receipt.correlation.workflow_id, 2);
    assert_eq!(
        s.fill(7, 0).unwrap().receipt.beneficiary,
        s.beneficiary.to_string()
    );
    assert_eq!(
        s.fill(8, 0).unwrap().receipt.beneficiary,
        s.sponsor.to_string()
    );
    assert!(s
        .app
        .execute_contract(
            s.sponsor.clone(),
            original.clone(),
            &ExecuteMsg::ReclaimPrincipal { order_id: 7 },
            &[]
        )
        .is_err());
    for id in [7, 8] {
        s.conservation(id);
    }
    s.contract = other;
    s.conservation(1);
    assert_eq!(s.fill(1, 0).unwrap().receipt.correlation.workflow_id, 1);
    let other_receipt = s.receipt(1, 0).unwrap();
    assert_eq!(other_receipt.order_id, 1);
    s.contract = original;
    assert_eq!(
        s.app
            .wrap()
            .query_balance(&s.beneficiary, "uoutput")
            .unwrap()
            .amount,
        Uint128::new(30)
    );
    assert_eq!(
        s.app
            .wrap()
            .query_balance(&s.sponsor, "uoutput")
            .unwrap()
            .amount,
        Uint128::new(1_000_015)
    );
    s.block(7).unwrap();
    assert_eq!(s.task(7).status, TaskStatus::Completed);
    assert_eq!(s.task(8).status, TaskStatus::Completed);
}

#[test]
fn minimum_output_failure_rolls_back_and_stays_paused() {
    let mut s = DcaSuite::new();
    let mut request = s.request();
    request.minimum_output = Uint128::new(16);
    s.create(s.owner.clone(), request).unwrap();
    s.block(2).unwrap();
    let before = business(&s);
    s.block(6).unwrap();
    assert_eq!(business(&s), before);
    assert_eq!(s.order(1).order.spent, Uint128::zero());
    assert!(s.receipt(1, 0).is_none());
    assert!(matches!(
        s.task(1).status,
        TaskStatus::Paused {
            reason: PauseReason::MessageFailure,
            ..
        }
    ));
    assert_eq!(s.block(50).unwrap().polls, 0);
    s.conservation(1);
}

#[test]
fn shared_input_or_output_gas_denom_keeps_trade_principal_separate() {
    for fee_denom in ["uinput", "uoutput"] {
        let mut s = DcaSuite::with_fee_denom(fee_denom);
        s.create(s.owner.clone(), s.request()).unwrap();
        s.block(2).unwrap();
        s.conservation(1);
        s.block(6).unwrap();
        s.conservation(1);
        assert_eq!(
            (s.order(1).order.spent, s.order(1).order.remaining),
            (Uint128::new(10), Uint128::new(20))
        );
        assert_eq!(
            s.app
                .wrap()
                .query_balance(&s.contract, "uinput")
                .unwrap()
                .amount,
            Uint128::new(20)
        );
        assert_eq!(
            s.app
                .wrap()
                .query_balance(&s.beneficiary, "uoutput")
                .unwrap()
                .amount,
            Uint128::new(15)
        );
        assert_eq!(s.funding(1).cumulative_execution_fees, Uint128::new(23));
        assert_eq!(s.funding(1).available, Uint128::new(976));
        manage(
            &mut s,
            s_owner(),
            AsyncExecuteMsg::CancelTask { task_id: 1 },
            &[],
        );
        s.app
            .execute_contract(
                s.owner.clone(),
                s.contract.clone(),
                &ExecuteMsg::ReclaimPrincipal { order_id: 1 },
                &[],
            )
            .unwrap();
        assert_eq!(s.order(1).order.reclaimed, Uint128::new(20));
        s.conservation(1);
    }
}

#[test]
fn cancel_after_receipt_does_not_refund_spent_slice() {
    // Catches refunding a committed slice whose inbox has not been consumed.
    let mut s = DcaSuite::new();
    s.create(s.owner.clone(), s.request()).unwrap();
    s.block(2).unwrap();
    s.block(6).unwrap();
    let order = s.order(1).order;
    assert_eq!(
        (order.filled_count, order.spent, order.remaining),
        (1, Uint128::new(10), Uint128::new(20))
    );
    assert_eq!(
        s.app
            .wrap()
            .query_balance(&s.beneficiary, "uoutput")
            .unwrap()
            .amount,
        Uint128::new(15)
    );
    assert!(s.task(1).inbox.is_some());
    s.app
        .execute_contract(
            s.owner.clone(),
            s.contract.clone(),
            &ExecuteMsg::Async(AsyncExecuteMsg::CancelTask { task_id: 1 }),
            &[],
        )
        .unwrap();
    s.app
        .execute_contract(
            s.owner.clone(),
            s.contract.clone(),
            &ExecuteMsg::ReclaimPrincipal { order_id: 1 },
            &[],
        )
        .unwrap();
    let order = s.order(1).order;
    assert_eq!(order.reclaimed, Uint128::new(20));
    assert_eq!(
        order.initial_principal,
        order.spent + order.remaining + order.reclaimed
    );
    let receipt: Option<dca_example_protocol::SwapReceipt> = s
        .app
        .wrap()
        .query_wasm_smart(
            &s.exchange,
            &dca_example_protocol::ExchangeQueryMsg::Receipt {
                dca: s.contract.to_string(),
                order_id: 1,
                slice_index: 0,
            },
        )
        .unwrap();
    assert!(receipt.is_some());
    s.block(7).unwrap();
    assert_eq!(s.order(1).order.filled_count, 1);
}
