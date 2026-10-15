#![allow(deprecated)]
mod support;

use cosmwasm_std::{
    testing::{mock_env, MockApi, MockQuerier, MockStorage},
    *,
};
use cw_multi_test::Executor;
use native_async::*;
use native_dca::{
    contract::{MIN_ALLOCATED_BYTES, REGISTER_REPLY},
    msg::*,
    state::*,
    *,
};
use std::marker::PhantomData;
use support::DcaSuite;

fn deps() -> OwnedDeps<MockStorage, MockApi, MockQuerier<RuntimeQuery>, RuntimeQuery> {
    OwnedDeps {
        storage: MockStorage::new(),
        api: MockApi::default(),
        querier: MockQuerier::new(&[]),
        custom_query_type: PhantomData,
    }
}

fn host_task(env: &Env, order_id: u64, task_id: u64) -> TaskResponse {
    TaskResponse {
        contract: env.contract.address.to_string(),
        task_id,
        program: TaskProgram {
            kind: "native_dca".into(),
            version: 1,
            input: to_json_binary(&WorkflowInput { order_id }).unwrap(),
        },
        code_checksum: Binary::from(vec![7; 32]),
        status: TaskStatus::Runnable,
        wait_sequence: 0,
        state: None,
        wait: None,
        inbox: None,
        output: None,
        error: None,
        last_attempt: None,
        expires_at: 100,
        retain_until: 110,
        allocated_bytes: MIN_ALLOCATED_BYTES,
    }
}

fn setup_unit() -> (
    OwnedDeps<MockStorage, MockApi, MockQuerier<RuntimeQuery>, RuntimeQuery>,
    Env,
    MessageInfo,
    CreateOrderRequest,
) {
    let mut deps = deps();
    let mut env = mock_env();
    env.block.height = 1;
    let owner = deps.api.addr_make("unit-owner");
    let exchange = deps.api.addr_make("unit-exchange");
    let beneficiary = deps.api.addr_make("unit-beneficiary");
    deps.querier = deps.querier.with_custom_handler(|query| match query {
        RuntimeQuery::Funding(SchedulerQuery::FundingConfig {}) => {
            SystemResult::Ok(ContractResult::Ok(
                to_json_binary(&FundingConfig {
                    denom: "ugas".into(),
                    gas_price: Decimal::one(),
                    min_base_price: Decimal::one(),
                    target_gas: 1,
                    price_grace_blocks: 1,
                    last_block_poll_gas_used: 0,
                    max_gas_per_poll: 1_000,
                    max_lifetime_blocks: 1_000,
                })
                .unwrap(),
            ))
        }
        _ => SystemResult::Ok(ContractResult::Err("temporary host outage".into())),
    });
    deps.querier.update_wasm(move |query| match query {
        WasmQuery::Smart { contract_addr, .. } if contract_addr == exchange.as_str() => {
            SystemResult::Ok(ContractResult::Ok(
                to_json_binary(&dca_example_protocol::ExchangeConfig {
                    input_denom: "uinput".into(),
                    output_denom: "uoutput".into(),
                    numerator: Uint128::new(3),
                    denominator: Uint128::new(2),
                })
                .unwrap(),
            ))
        }
        _ => SystemResult::Ok(ContractResult::Err("unknown exchange".into())),
    });
    let exchange = deps.api.addr_make("unit-exchange");
    instantiate(
        deps.as_mut(),
        env.clone(),
        MessageInfo {
            sender: owner.clone(),
            funds: vec![],
        },
        InstantiateMsg {
            exchange: exchange.to_string(),
            fee_denom: "ugas".into(),
        },
    )
    .unwrap();
    let request = CreateOrderRequest {
        input_per_slice: coin(10, "uinput"),
        slice_count: 3,
        minimum_output: Uint128::new(14),
        interval_blocks: 5,
        beneficiary: beneficiary.to_string(),
        gas: coin(1_000, "ugas"),
        gas_per_poll: 100,
        max_gas_price: Decimal::one(),
        expires_at: 100,
        allocated_bytes: MIN_ALLOCATED_BYTES,
    };
    (
        deps,
        env,
        MessageInfo {
            sender: owner,
            funds: vec![coin(30, "uinput"), coin(1_000, "ugas")],
        },
        request,
    )
}

fn registration_reply(payload: Binary, data: Option<Binary>) -> Reply {
    Reply {
        id: REGISTER_REPLY,
        payload,
        gas_used: 0,
        result: SubMsgResult::Ok(SubMsgResponse {
            events: vec![],
            data,
            msg_responses: vec![],
        }),
    }
}

#[test]
fn registration_reply_uses_host_id_not_reply_tag() {
    // Catches binding reply tag/local order 7 as task id rather than host id 1.
    let mut suite = DcaSuite::new();
    suite
        .app
        .contract_storage_mut(&suite.contract)
        .set(NEXT_ORDER_ID_KEY, &to_json_vec(&7u64).unwrap());
    let result = suite
        .create(suite.sponsor.clone(), suite.request())
        .unwrap();
    assert_eq!(
        result,
        CreateOrderResponse {
            order_id: 7,
            task_id: 1
        }
    );
    assert_eq!(suite.order(7).order.owner, suite.sponsor);
    assert_eq!(suite.task(7).task_id, 1);
    assert_eq!(suite.funding(1).refund, suite.sponsor.to_string());

    let (mut deps, env, info, request) = setup_unit();
    deps.storage
        .set(NEXT_ORDER_ID_KEY, &to_json_vec(&7u64).unwrap());
    let response = execute(
        deps.as_mut(),
        env.clone(),
        info.clone(),
        ExecuteMsg::CreateOrder(request),
    )
    .unwrap();
    assert_eq!(response.messages[0].id, 1);
    assert_eq!(
        response.messages[0].payload,
        Binary::from(br#"{"order_id":7}"#.as_slice())
    );
    let task = host_task(&env, 7, 1);
    deps.querier = deps.querier.with_custom_handler(move |query| match query {
        RuntimeQuery::Task { contract, task_id } if contract == &task.contract && *task_id == 1 => {
            SystemResult::Ok(ContractResult::Ok(to_json_binary(&task).unwrap()))
        }
        _ => SystemResult::Ok(ContractResult::Err("wrong host lookup".into())),
    });
    let result = reply(
        deps.as_mut(),
        env.clone(),
        registration_reply(
            response.messages[0].payload.clone(),
            Some(Binary::from(b"1".as_slice())),
        ),
    )
    .unwrap();
    assert_eq!(
        from_json::<CreateOrderResponse>(result.data.unwrap()).unwrap(),
        CreateOrderResponse {
            order_id: 7,
            task_id: 1
        }
    );
    let order = load_order(&deps.storage, 7).unwrap();
    assert_eq!(
        (order.order_id, order.task_id, order.owner),
        (7, Some(1), info.sender)
    );
    assert!(reply(
        deps.as_mut(),
        env,
        registration_reply(
            response.messages[0].payload.clone(),
            Some(Binary::from(b"1".as_slice()))
        )
    )
    .is_err());
}

#[test]
fn registration_reply_rejects_missing_malformed_and_cross_order_payloads() {
    // Catches missing correlation or binding another registered program/input.
    let (mut deps, env, info, request) = setup_unit();
    execute(
        deps.as_mut(),
        env.clone(),
        info.clone(),
        ExecuteMsg::CreateOrder(request.clone()),
    )
    .unwrap();
    execute(
        deps.as_mut(),
        env.clone(),
        info,
        ExecuteMsg::CreateOrder(request),
    )
    .unwrap();
    let task = host_task(&env, 1, 41);
    deps.querier = deps.querier.with_custom_handler(move |_| {
        SystemResult::Ok(ContractResult::Ok(to_json_binary(&task).unwrap()))
    });
    let good_payload = to_json_binary(&RegisterReplyPayload { order_id: 1 }).unwrap();
    for payload in [
        Binary::default(),
        Binary::from(b"broken".as_slice()),
        to_json_binary(&RegisterReplyPayload { order_id: 2 }).unwrap(),
    ] {
        assert!(reply(
            deps.as_mut(),
            env.clone(),
            registration_reply(payload, Some(Binary::from(b"41".as_slice())))
        )
        .is_err());
    }
    for data in [
        None,
        Some(Binary::from(b"broken".as_slice())),
        Some(Binary::from(br#"{"task_id":41}"#.as_slice())),
    ] {
        assert!(reply(
            deps.as_mut(),
            env.clone(),
            registration_reply(good_payload.clone(), data)
        )
        .is_err());
    }
    for mutation in 0..4 {
        let mut task = host_task(&env, 1, 41);
        match mutation {
            0 => task.program.kind = "foreign".into(),
            1 => task.program.version = 2,
            2 => task.contract = "foreign".into(),
            _ => task.task_id = 99,
        }
        deps.querier = deps.querier.with_custom_handler(move |_| {
            SystemResult::Ok(ContractResult::Ok(to_json_binary(&task).unwrap()))
        });
        assert!(reply(
            deps.as_mut(),
            env.clone(),
            registration_reply(good_payload.clone(), Some(Binary::from(b"41".as_slice())))
        )
        .is_err());
    }
    assert_eq!(load_order(&deps.storage, 1).unwrap().task_id, None);
    assert_eq!(load_order(&deps.storage, 2).unwrap().task_id, None);
    let mut unexpected = registration_reply(good_payload, Some(Binary::from(b"41".as_slice())));
    unexpected.id = 2;
    assert!(reply(deps.as_mut(), env, unexpected).is_err());
}

#[test]
fn host_failure_before_expiry_is_unavailable_and_cannot_release_principal() {
    // Catches interpreting arbitrary transient errors as pruned/terminal state.
    let (mut deps, mut env, info, request) = setup_unit();
    execute(
        deps.as_mut(),
        env.clone(),
        info.clone(),
        ExecuteMsg::CreateOrder(request),
    )
    .unwrap();
    let mut order = load_order(&deps.storage, 1).unwrap();
    order.task_id = Some(41);
    save_order(&mut deps.storage, &order).unwrap();
    let view: OrderView =
        from_json(query(deps.as_ref(), env.clone(), QueryMsg::Order { order_id: 1 }).unwrap())
            .unwrap();
    assert!(
        matches!(view.host, HostTaskView::Unavailable { error, lease_expired: false } if error.contains("temporary host outage"))
    );
    assert!(
        query(deps.as_ref(), env.clone(), QueryMsg::Task { order_id: 1 })
            .unwrap_err()
            .to_string()
            .contains("temporary host outage")
    );
    let owner = MessageInfo {
        sender: info.sender,
        funds: vec![],
    };
    assert!(execute(
        deps.as_mut(),
        env.clone(),
        owner.clone(),
        ExecuteMsg::ReclaimPrincipal { order_id: 1 }
    )
    .unwrap_err()
    .to_string()
    .contains("temporary host outage"));
    assert_eq!(load_order(&deps.storage, 1).unwrap(), order);
    env.block.height = 100;
    assert!(execute(
        deps.as_mut(),
        env,
        owner,
        ExecuteMsg::ReclaimPrincipal { order_id: 1 }
    )
    .is_ok());
}

#[test]
fn duplicate_zero_funds_and_checked_coalescing_are_rejected() {
    // Bare handler checks malformed fund vectors that SDK normalization rejects earlier.
    let (mut deps, env, info, request) = setup_unit();
    for funds in [
        vec![coin(30, "uinput"), coin(500, "ugas"), coin(500, "ugas")],
        vec![coin(30, "uinput"), coin(1_000, "ugas"), coin(0, "uoutput")],
    ] {
        assert!(execute(
            deps.as_mut(),
            env.clone(),
            MessageInfo {
                sender: info.sender.clone(),
                funds
            },
            ExecuteMsg::CreateOrder(request.clone())
        )
        .is_err());
    }
    let mut request = request;
    request.input_per_slice.amount = Uint128::MAX;
    request.slice_count = 1;
    request.gas = coin(1, "uinput");
    // Configured denom mismatch is separately covered; set this fixture to shared denom.
    let mut config = load_config(&deps.storage).unwrap();
    config.fee_denom = "uinput".into();
    deps.storage.set(CONFIG_KEY, &to_json_vec(&config).unwrap());
    let error = execute(deps.as_mut(), env, info, ExecuteMsg::CreateOrder(request)).unwrap_err();
    assert!(
        matches!(
            error,
            native_dca::error::ContractError::Std(StdError::Overflow { .. })
        ),
        "{error:?}"
    );
}

#[test]
fn callback_envelope_matches_shared_protocol() {
    let receipt: dca_example_protocol::SwapReceipt = from_json(include_bytes!(
        "../../dca-protocol/tests/fixtures/receipt-2048.json"
    ))
    .unwrap();
    assert_eq!(
        to_json_binary(&ExecuteMsg::SwapResult {
            receipt: receipt.clone()
        })
        .unwrap(),
        to_json_binary(&dca_example_protocol::DcaCallbackMsg::SwapResult { receipt }).unwrap()
    );
}

#[test]
fn deployment_rejects_host_fee_mismatch_invalid_exchange_and_funds() {
    // Catches trusting deployment labels instead of querying both immutable configs.
    let (mut deps, env, info, _) = setup_unit();
    let exchange = deps.api.addr_make("unit-exchange").to_string();
    assert!(instantiate(
        deps.as_mut(),
        env.clone(),
        MessageInfo {
            funds: vec![],
            ..info.clone()
        },
        InstantiateMsg {
            exchange: exchange.clone(),
            fee_denom: "uother".into()
        }
    )
    .is_err());
    assert!(instantiate(
        deps.as_mut(),
        env.clone(),
        info.clone(),
        InstantiateMsg {
            exchange: exchange.clone(),
            fee_denom: "ugas".into()
        }
    )
    .is_err());
    assert!(instantiate(
        deps.as_mut(),
        env.clone(),
        MessageInfo {
            funds: vec![],
            ..info.clone()
        },
        InstantiateMsg {
            exchange: "bad-address".into(),
            fee_denom: "ugas".into()
        }
    )
    .is_err());
    for mutation in 0..5 {
        let mut terms = dca_example_protocol::ExchangeConfig {
            input_denom: "uinput".into(),
            output_denom: "uoutput".into(),
            numerator: Uint128::one(),
            denominator: Uint128::one(),
        };
        match mutation {
            0 => terms.input_denom = "bad!".into(),
            1 => terms.output_denom = "bad!".into(),
            2 => terms.output_denom = "uinput".into(),
            3 => terms.numerator = Uint128::zero(),
            _ => terms.denominator = Uint128::zero(),
        }
        deps.querier.update_wasm(move |_| {
            SystemResult::Ok(ContractResult::Ok(to_json_binary(&terms).unwrap()))
        });
        assert!(instantiate(
            deps.as_mut(),
            env.clone(),
            MessageInfo {
                funds: vec![],
                ..info.clone()
            },
            InstantiateMsg {
                exchange: exchange.clone(),
                fee_denom: "ugas".into()
            }
        )
        .is_err());
    }
}

#[test]
fn exhausted_order_id_and_principal_product_do_not_write_orders() {
    // Catches wrapping IDs onto existing custody or truncating a u128 product.
    let (mut deps, env, info, request) = setup_unit();
    deps.storage
        .set(NEXT_ORDER_ID_KEY, &to_json_vec(&u64::MAX).unwrap());
    assert!(execute(
        deps.as_mut(),
        env.clone(),
        info.clone(),
        ExecuteMsg::CreateOrder(request.clone())
    )
    .is_err());
    assert!(load_order(&deps.storage, u64::MAX).is_err());
    let mut request = request;
    request.input_per_slice.amount = Uint128::MAX;
    let error = execute(deps.as_mut(), env, info, ExecuteMsg::CreateOrder(request)).unwrap_err();
    assert!(matches!(
        error,
        native_dca::error::ContractError::Std(StdError::Overflow { .. })
    ));
}

#[test]
fn nonfunding_calls_reject_attached_coins() {
    // Catches accidental principal/gas credit through management or callback APIs.
    let (mut deps, env, info, request) = setup_unit();
    execute(
        deps.as_mut(),
        env.clone(),
        info.clone(),
        ExecuteMsg::CreateOrder(request),
    )
    .unwrap();
    let receipt: dca_example_protocol::SwapReceipt = from_json(include_bytes!(
        "../../dca-protocol/tests/fixtures/receipt-2048.json"
    ))
    .unwrap();
    let calls = [
        ExecuteMsg::UpdateBudget {
            order_id: 1,
            gas_per_poll: 100,
            max_gas_price: Decimal::one(),
        },
        ExecuteMsg::ReclaimPrincipal { order_id: 1 },
        ExecuteMsg::Async(AsyncExecuteMsg::CancelTask { task_id: 1 }),
        ExecuteMsg::Async(AsyncExecuteMsg::ResumeTask { task_id: 1 }),
        ExecuteMsg::DispatchSlice {
            order_id: 1,
            slice_index: 0,
            correlation: dca_example_protocol::Correlation {
                workflow_id: 1,
                wait_sequence: 1,
            },
        },
        ExecuteMsg::SwapResult { receipt },
    ];
    for msg in calls {
        assert!(execute(deps.as_mut(), env.clone(), info.clone(), msg).is_err());
    }
}

#[test]
fn maximum_width_checkpoint_envelope_and_host_wrappers_fit_allocation() {
    // Catches underallocating the declared continuation envelope after base64 and
    // maximum-width host counters. This is not the generated-state proof (Task 5).
    use native_async::runtime::{ExecutionStatus, ExecutionTask};
    let mut suite = DcaSuite::new();
    suite.create(suite.owner.clone(), suite.request()).unwrap();
    let key = [
        b"na2/task/".as_slice(),
        suite.contract.as_bytes(),
        b"/",
        &1u64.to_be_bytes(),
    ]
    .concat();
    let mut task: ExecutionTask = from_json(suite.app.storage().get(&key).unwrap()).unwrap();
    let mut funding = suite.funding(1);
    let address = "x".repeat(256); // Deliberately wider than fixture addresses.
    task.contract = address.clone();
    task.task_id = u64::MAX;
    task.program.input = to_json_binary(&WorkflowInput { order_id: u64::MAX }).unwrap();
    task.code_checksum = Binary::from(vec![255; 32]);
    task.state = Some(Binary::from(vec![
        255;
        native_dca::contract::MAX_CHECKPOINT_BYTES
    ]));
    task.wait = Some(Wait::Operation {
        resolver: address.clone(),
        deadline: u64::MAX,
        max_result_bytes: 2048,
    });
    task.inbox = None;
    task.wait_sequence = u64::MAX;
    task.status = ExecutionStatus::Completed;
    task.allocated_bytes = u64::MAX;
    task.storage_fee = Uint128::MAX;
    task.charged = Uint128::MAX;
    task.reserved = Uint128::MAX;
    task.last_attempt = Some(u64::MAX);
    task.ready_at = u64::MAX;
    task.expires_at = u64::MAX;
    task.retain_until = u64::MAX;
    task.output = Some(Binary::from(vec![255; 256]));
    task.error = Some("\"\\".repeat(128));
    funding.task_id = u64::MAX;
    funding.contract = address.clone();
    funding.control = address.clone();
    funding.refund = address;
    funding.available = Uint128::MAX;
    funding.reserved = Uint128::MAX;
    funding.cumulative_execution_fees = Uint128::MAX;
    funding.allocated_bytes = u64::MAX;
    funding.storage_lease_paid = Uint128::MAX;
    funding.gas_per_poll = u64::MAX;
    funding.max_gas_price = Decimal::MAX;
    funding.max_attempt_charge = Uint128::MAX;
    funding.expires_at = u64::MAX;
    funding.status = TaskStatus::Paused {
        reason: PauseReason::InsufficientFunds,
        resume: ResumeState::Runnable,
    };
    // The private queue/expiry keys are already maximum-width binary encodings.
    let state_and_wrappers =
        to_json_vec(&task).unwrap().len() + to_json_vec(&funding).unwrap().len();
    println!(
        "max-width checkpoint+host wrappers: {state_and_wrappers}; base64 checkpoint: {}",
        task.state.as_ref().unwrap().to_base64().len()
    );
    assert!(state_and_wrappers as u64 <= native_dca::contract::STATE_AND_HOST_ALLOWANCE);
    assert!(state_and_wrappers as u64 + 13_440 <= MIN_ALLOCATED_BYTES);
}

#[test]
fn exact_principal_stays_in_custody_and_only_gas_reaches_host() {
    // Catches forwarding principal to scheduler or using a synthetic owner.
    let mut suite = DcaSuite::new();
    let result = suite.create(suite.owner.clone(), suite.request()).unwrap();
    assert_eq!(
        result,
        CreateOrderResponse {
            order_id: 1,
            task_id: 1
        }
    );
    let order = suite.order(1).order;
    assert_eq!(order.owner, suite.owner);
    assert_eq!(order.beneficiary, suite.beneficiary);
    assert_eq!(
        (
            order.initial_principal,
            order.remaining,
            order.spent,
            order.reclaimed
        ),
        (
            Uint128::new(30),
            Uint128::new(30),
            Uint128::zero(),
            Uint128::zero()
        )
    );
    assert_eq!(
        suite
            .app
            .wrap()
            .query_balance(&suite.contract, "uinput")
            .unwrap(),
        coin(30, "uinput")
    );
    assert_eq!(
        suite
            .app
            .wrap()
            .query_balance(&suite.contract, "ugas")
            .unwrap(),
        coin(0, "ugas")
    );
    let funding = suite.funding(1);
    assert_eq!(
        (funding.control, funding.refund),
        (suite.owner.to_string(), suite.owner.to_string())
    );
    assert_eq!(funding.available, Uint128::new(999));
    assert_eq!(
        suite.task(1).program,
        TaskProgram {
            kind: "native_dca".into(),
            version: 1,
            input: Binary::from(br#"{"order_id":1}"#.as_slice())
        }
    );
    let fill: Option<FillRecord> = suite
        .app
        .wrap()
        .query_wasm_smart(
            &suite.contract,
            &QueryMsg::Fill {
                order_id: 1,
                slice_index: 0,
            },
        )
        .unwrap();
    assert_eq!(fill, None);
}

#[test]
fn slice_count_boundaries_admit_one_and_128_only() {
    // Catches accepting an empty/unbounded order or rejecting the upper boundary.
    for count in [0, 1, 128, 129] {
        let mut suite = DcaSuite::new();
        let mut request = suite.request();
        request.slice_count = count;
        let funds = if count == 0 {
            vec![coin(1_000, "ugas")]
        } else {
            suite.funds(&request)
        };
        let before = suite.app.wrap().query_all_balances(&suite.owner).unwrap();
        let result = suite.app.execute_contract(
            suite.owner.clone(),
            suite.contract.clone(),
            &ExecuteMsg::CreateOrder(request),
            &funds,
        );
        assert_eq!(
            result.is_ok(),
            count == 1 || count == 128,
            "slice count {count}: {result:?}"
        );
        if result.is_err() {
            assert_eq!(
                suite.app.wrap().query_all_balances(&suite.owner).unwrap(),
                before
            );
        }
    }
}

#[test]
fn invalid_admission_terms_and_exact_funds_are_rejected_atomically() {
    // Each mutation catches a missing admission check before principal is accepted.
    let mut suite = DcaSuite::new();
    let original = suite.request();
    let mut cases = Vec::new();
    let mut r = original.clone();
    r.input_per_slice.amount = Uint128::zero();
    cases.push(r);
    let mut r = original.clone();
    r.minimum_output = Uint128::zero();
    cases.push(r);
    let mut r = original.clone();
    r.interval_blocks = 0;
    cases.push(r);
    let mut r = original.clone();
    r.gas.amount = Uint128::zero();
    cases.push(r);
    let mut r = original.clone();
    r.gas_per_poll = 0;
    cases.push(r);
    let mut r = original.clone();
    r.max_gas_price = Decimal::zero();
    cases.push(r);
    let mut r = original.clone();
    r.expires_at = 0;
    cases.push(r);
    let mut r = original.clone();
    r.allocated_bytes = 0;
    cases.push(r);
    let mut r = original.clone();
    r.allocated_bytes = MIN_ALLOCATED_BYTES - 1;
    cases.push(r);
    let mut r = original.clone();
    r.input_per_slice.denom = "uwrong".into();
    cases.push(r);
    let mut r = original.clone();
    r.gas.denom = "uwrong".into();
    cases.push(r);
    let mut r = original.clone();
    r.beneficiary = "invalid".into();
    cases.push(r);
    let mut r = original.clone();
    r.expires_at = 1;
    cases.push(r);
    let mut r = original.clone();
    r.input_per_slice.amount = Uint128::MAX;
    cases.push(r);
    let before = suite.app.wrap().query_all_balances(&suite.owner).unwrap();
    for request in cases {
        let result = suite.app.execute_contract(
            suite.owner.clone(),
            suite.contract.clone(),
            &ExecuteMsg::CreateOrder(request),
            &[coin(30, "uinput"), coin(1_000, "ugas")],
        );
        assert!(result.is_err());
        assert_eq!(
            suite.app.wrap().query_all_balances(&suite.owner).unwrap(),
            before
        );
    }
    for funds in [
        vec![coin(30, "uinput")],
        vec![coin(31, "uinput"), coin(1_000, "ugas")],
        vec![coin(30, "uinput"), coin(1_001, "ugas")],
        vec![coin(30, "uinput"), coin(1_000, "ugas"), coin(1, "uoutput")],
    ] {
        assert!(suite
            .app
            .execute_contract(
                suite.owner.clone(),
                suite.contract.clone(),
                &ExecuteMsg::CreateOrder(original.clone()),
                &funds
            )
            .is_err());
    }
    assert_eq!(
        suite
            .create(suite.owner.clone(), original)
            .unwrap()
            .order_id,
        1
    );
}

#[test]
fn shared_input_or_output_gas_denom_preserves_principal_split() {
    // Catches double-counting/coalescing mistakes when gas uses an exchange denom.
    for denom in ["uinput", "uoutput"] {
        let mut suite = DcaSuite::with_fee_denom(denom);
        suite.create(suite.owner.clone(), suite.request()).unwrap();
        assert_eq!(
            suite
                .app
                .wrap()
                .query_balance(&suite.contract, "uinput")
                .unwrap(),
            coin(30, "uinput")
        );
        assert_eq!(suite.funding(1).available, Uint128::new(999));
        assert_eq!(suite.order(1).order.initial_principal, Uint128::new(30));
    }
}

#[test]
fn owner_management_sponsor_funding_and_two_orders_are_isolated() {
    // Catches sponsor authority escalation, refund rebinding and cross-order drain.
    let mut suite = DcaSuite::new();
    suite.create(suite.owner.clone(), suite.request()).unwrap();
    suite
        .create(suite.sponsor.clone(), suite.request())
        .unwrap();
    let budget = ExecuteMsg::UpdateBudget {
        order_id: 1,
        gas_per_poll: 200,
        max_gas_price: Decimal::percent(150),
    };
    assert!(suite
        .app
        .execute_contract(suite.sponsor.clone(), suite.contract.clone(), &budget, &[])
        .is_err());
    assert!(suite
        .app
        .execute_contract(
            suite.owner.clone(),
            suite.contract.clone(),
            &budget,
            &[coin(1, "ugas")]
        )
        .is_err());
    suite
        .app
        .execute_contract(suite.owner.clone(), suite.contract.clone(), &budget, &[])
        .unwrap();
    assert_eq!(suite.funding(1).refund, suite.owner.to_string());
    assert_eq!(suite.funding(1).gas_per_poll, 200);
    suite
        .app
        .execute_contract(
            suite.sponsor.clone(),
            suite.contract.clone(),
            &ExecuteMsg::Async(AsyncExecuteMsg::FundTask { task_id: 1 }),
            &[coin(100, "ugas")],
        )
        .unwrap();
    assert_eq!(suite.funding(1).available, Uint128::new(1_099));
    for msg in [
        ExecuteMsg::Async(AsyncExecuteMsg::CancelTask { task_id: 1 }),
        ExecuteMsg::Async(AsyncExecuteMsg::ResumeTask { task_id: 1 }),
        ExecuteMsg::ReclaimPrincipal { order_id: 1 },
    ] {
        assert!(suite
            .app
            .execute_contract(suite.sponsor.clone(), suite.contract.clone(), &msg, &[])
            .is_err());
    }
    assert!(suite
        .app
        .execute_contract(
            suite.owner.clone(),
            suite.contract.clone(),
            &ExecuteMsg::ReclaimPrincipal { order_id: 1 },
            &[]
        )
        .is_err());
    suite
        .app
        .execute_contract(
            suite.owner.clone(),
            suite.contract.clone(),
            &ExecuteMsg::Async(AsyncExecuteMsg::CancelTask { task_id: 1 }),
            &[],
        )
        .unwrap();
    suite
        .app
        .send_tokens(
            suite.owner.clone(),
            suite.contract.clone(),
            &[coin(7, "uinput"), coin(8, "ugas")],
        )
        .unwrap();
    assert_eq!(suite.funding(1).available, Uint128::zero());
    suite
        .app
        .execute_contract(
            suite.owner.clone(),
            suite.contract.clone(),
            &ExecuteMsg::ReclaimPrincipal { order_id: 1 },
            &[],
        )
        .unwrap();
    assert_eq!(
        (
            suite.order(1).order.remaining,
            suite.order(1).order.reclaimed
        ),
        (Uint128::zero(), Uint128::new(30))
    );
    assert_eq!(suite.order(2).order.remaining, Uint128::new(30));
    assert_eq!(
        suite
            .app
            .wrap()
            .query_balance(&suite.contract, "uinput")
            .unwrap(),
        coin(37, "uinput")
    );
    assert!(suite
        .app
        .execute_contract(
            suite.owner.clone(),
            suite.contract.clone(),
            &ExecuteMsg::ReclaimPrincipal { order_id: 1 },
            &[]
        )
        .is_err());
}

#[test]
fn failed_bank_reclaim_rolls_back_accounting() {
    // Catches marking a claim reclaimed when its outgoing bank send fails.
    let mut suite = DcaSuite::new();
    suite.create(suite.owner.clone(), suite.request()).unwrap();
    suite
        .app
        .execute_contract(
            suite.owner.clone(),
            suite.contract.clone(),
            &ExecuteMsg::Async(AsyncExecuteMsg::CancelTask { task_id: 1 }),
            &[],
        )
        .unwrap();
    // Deliberately remove custody in the harness to make the real bank send fail.
    suite
        .app
        .send_tokens(
            suite.contract.clone(),
            suite.sponsor.clone(),
            &[coin(30, "uinput")],
        )
        .unwrap();
    let before = suite.order(1).order;
    assert!(suite
        .app
        .execute_contract(
            suite.owner.clone(),
            suite.contract.clone(),
            &ExecuteMsg::ReclaimPrincipal { order_id: 1 },
            &[]
        )
        .is_err());
    assert_eq!(suite.order(1).order, before);
}

#[test]
fn immutable_expiry_allows_reclaim_after_host_pruning() {
    // Catches using a missing task as early completion or requiring it after expiry.
    let mut suite = DcaSuite::new();
    suite.create(suite.owner.clone(), suite.request()).unwrap();
    suite.block(111).unwrap();
    assert!(suite
        .app
        .wrap()
        .query_wasm_smart::<TaskResponse>(&suite.contract, &QueryMsg::Task { order_id: 1 })
        .is_err());
    assert!(matches!(
        suite.order(1).host,
        HostTaskView::Unavailable {
            lease_expired: true,
            ..
        }
    ));
    suite
        .app
        .execute_contract(
            suite.owner.clone(),
            suite.contract.clone(),
            &ExecuteMsg::ReclaimPrincipal { order_id: 1 },
            &[],
        )
        .unwrap();
    assert_eq!(suite.order(1).order.reclaimed, Uint128::new(30));
}

#[test]
fn failed_host_registration_rolls_back_order_ids_principal_and_gas() {
    // Catches writes/funds escaping a rejected native registration transaction.
    let mut suite = DcaSuite::new();
    let mut request = suite.request();
    request.expires_at = 2_000;
    let owner_before = suite.app.wrap().query_all_balances(&suite.owner).unwrap();
    assert!(suite.create(suite.owner.clone(), request).is_err());
    assert_eq!(
        suite.app.wrap().query_all_balances(&suite.owner).unwrap(),
        owner_before
    );
    assert_eq!(
        suite
            .app
            .wrap()
            .query_all_balances(&suite.contract)
            .unwrap(),
        vec![]
    );
    assert!(suite
        .app
        .wrap()
        .query_wasm_smart::<OrderView>(&suite.contract, &QueryMsg::Order { order_id: 1 })
        .is_err());
    assert_eq!(
        suite.create(suite.owner.clone(), suite.request()).unwrap(),
        CreateOrderResponse {
            order_id: 1,
            task_id: 1
        }
    );
}
