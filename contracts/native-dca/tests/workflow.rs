mod support;
use cosmwasm_std::*;
use native_async::*;
use native_dca::{
    msg::*,
    state::*,
    workflow::{DcaOutcome, DcaWorkflowError},
};
use support::DcaSuite;

#[test]
fn strict_timer_and_operation_expiry_bounds_and_height_overflow() {
    // Catches equality acceptance, overflow, and stale pre-timer lease checks.
    for (expiry, interval, resume_height) in [(6, 5, 2), (7, 5, 6), (10, 5, 9), (100, u64::MAX, 2)]
    {
        let mut s = DcaSuite::new();
        let mut request = s.request();
        request.expires_at = expiry;
        request.interval_blocks = interval;
        s.create(s.owner.clone(), request).unwrap();
        s.block(2).unwrap();
        if resume_height != 2 {
            s.block(resume_height).unwrap();
        }
        let task = s.task(1);
        assert_eq!(
            task.status,
            TaskStatus::Completed,
            "expiry={expiry} interval={interval} height={resume_height}"
        );
        assert_eq!(
            from_json::<DcaOutcome>(task.output.unwrap()).unwrap(),
            DcaOutcome::LeaseWindowExhausted
        );
        assert_eq!(s.order(1).order.filled_count, 0);
        assert!(s.receipt(1, 0).is_none());
        s.conservation(1);
    }
}

fn bridge(deps: DepsMut, env: Env, task_id: u64, sequence: u64, step: PollStep) -> PollResponse {
    let response = native_dca::sudo(
        deps,
        env,
        TaskSudoMsg::PollTask(PollRequest {
            task_id,
            wait_sequence: sequence,
            kind: "native_dca".into(),
            version: 1,
            step,
        }),
    )
    .unwrap();
    assert!(response.messages.is_empty());
    from_json(response.data.unwrap()).unwrap()
}

#[test]
fn every_generated_checkpoint_and_terminal_encoding_fits_minimum_allocation() {
    // Catches checkpoint growth beyond admission's reserved envelope. Both real
    // generated wait tags are reached; numeric widths exceed admitted slice limits.
    use cosmwasm_std::testing::{mock_dependencies, mock_env};
    use durable_runtime::{Operation, Workflow};
    use native_async::runtime::{ExecutionStatus, ExecutionTask};
    let mut s = DcaSuite::new();
    s.create(s.owner.clone(), s.request()).unwrap();
    let mut order = s.order(1).order;
    order.order_id = u64::MAX;
    order.task_id = Some(u64::MAX);
    order.filled_count = u32::MAX - 1;
    order.slice_count = u32::MAX;
    order.registration_height = 1;
    order.interval_blocks = u64::MAX - 100;
    order.expires_at = u64::MAX;
    let mut deps = mock_dependencies();
    save_order(&mut deps.storage, &order).unwrap();
    let mut env = mock_env();
    env.block.height = 1;
    let timer = bridge(
        deps.as_mut(),
        env.clone(),
        u64::MAX,
        0,
        PollStep::Start {
            input: to_json_binary(&WorkflowInput { order_id: u64::MAX }).unwrap(),
        },
    );
    let PollOutcome::Pending {
        state: timer_state,
        wait: timer_wait,
    } = timer.outcome
    else {
        panic!("timer checkpoint missing");
    };
    assert_eq!(
        durable_runtime::native::SLEEP_BLOCKS_KIND,
        native_dca::workflow::native_dca::Workflow::operation(&timer_state).unwrap()
    );
    env.block.height = u64::MAX - 99;
    let swap = bridge(
        deps.as_mut(),
        env.clone(),
        u64::MAX,
        u64::MAX - 1,
        PollStep::Resume {
            state: timer_state.clone(),
            resolution: Resolution::Timer {},
        },
    );
    let PollOutcome::Pending {
        state: swap_state,
        wait: swap_wait,
    } = swap.outcome
    else {
        panic!("operation checkpoint missing");
    };
    assert_eq!(
        native_dca::workflow::SwapSlice::KIND,
        native_dca::workflow::native_dca::Workflow::operation(&swap_state).unwrap()
    );
    let mut cases = vec![
        ("timer", Some(timer_state), Some(timer_wait), None, None),
        (
            "operation",
            Some(swap_state.clone()),
            Some(swap_wait),
            None,
            None,
        ),
    ];
    let mut missing = mock_dependencies();
    let response = bridge(
        missing.as_mut(),
        env.clone(),
        u64::MAX,
        0,
        PollStep::Start {
            input: to_json_binary(&WorkflowInput { order_id: u64::MAX }).unwrap(),
        },
    );
    let PollOutcome::Fail { error } = response.outcome else {
        panic!("missing order was not diagnosed");
    };
    assert_eq!(
        from_json::<DcaWorkflowError>(error.as_bytes()).unwrap(),
        DcaWorkflowError::Storage
    );
    assert_eq!(error.len(), 9);
    cases.push(("storage diagnostic", None, None, None, Some(error)));
    for (resolution, expected) in [
        (Resolution::Timeout {}, DcaWorkflowError::Timeout),
        (
            Resolution::Error {
                code: "\"\\".repeat(10_000),
                message: "\"\\".repeat(10_000),
            },
            DcaWorkflowError::Remote,
        ),
    ] {
        let response = bridge(
            deps.as_mut(),
            env.clone(),
            u64::MAX,
            u64::MAX - 1,
            PollStep::Resume {
                state: swap_state.clone(),
                resolution,
            },
        );
        assert!(response.messages.is_empty());
        let PollOutcome::Fail { error } = response.outcome else {
            panic!("unexpected retry");
        };
        assert_eq!(
            from_json::<DcaWorkflowError>(error.as_bytes()).unwrap(),
            expected
        );
        assert!(error.len() <= 9);
        cases.push(("diagnostic", None, None, None, Some(error)));
    }
    for (filled_count, interval, expected) in [
        (u32::MAX, 1, DcaOutcome::Filled),
        (u32::MAX - 1, u64::MAX, DcaOutcome::LeaseWindowExhausted),
    ] {
        order.filled_count = filled_count;
        order.interval_blocks = interval;
        save_order(&mut deps.storage, &order).unwrap();
        let response = bridge(
            deps.as_mut(),
            env.clone(),
            u64::MAX,
            0,
            PollStep::Start {
                input: to_json_binary(&WorkflowInput { order_id: u64::MAX }).unwrap(),
            },
        );
        let PollOutcome::Complete { output } = response.outcome else {
            panic!("terminal output missing");
        };
        assert_eq!(from_json::<DcaOutcome>(&output).unwrap(), expected);
        cases.push(("terminal", None, None, Some(output), None));
    }
    // A host cannot poll at its already expired maximum height, but the pure
    // workflow boundary still rejects H+1 overflow without constructing a wait.
    env.block.height = u64::MAX;
    order.interval_blocks = 1;
    save_order(&mut deps.storage, &order).unwrap();
    let response = bridge(
        deps.as_mut(),
        env,
        u64::MAX,
        0,
        PollStep::Start {
            input: to_json_binary(&WorkflowInput { order_id: u64::MAX }).unwrap(),
        },
    );
    let PollOutcome::Complete { output } = response.outcome else {
        panic!("height overflow prepared a wait");
    };
    assert_eq!(
        from_json::<DcaOutcome>(output).unwrap(),
        DcaOutcome::LeaseWindowExhausted
    );
    let key = [
        b"na2/task/".as_slice(),
        s.contract.as_bytes(),
        b"/",
        &1u64.to_be_bytes(),
    ]
    .concat();
    let baseline: ExecutionTask = from_json(s.app.storage().get(&key).unwrap()).unwrap();
    for (name, state, wait, output, error) in cases {
        if let Some(state) = &state {
            assert!(state.len() <= native_dca::contract::MAX_CHECKPOINT_BYTES);
            println!("actual {name} raw checkpoint bytes: {}", state.len());
        }
        let mut task = baseline.clone();
        let mut funding = s.funding(1);
        let address = "x".repeat(256);
        task.contract = address.clone();
        task.task_id = u64::MAX;
        task.program.input = to_json_binary(&WorkflowInput { order_id: u64::MAX }).unwrap();
        task.code_checksum = Binary::from(vec![255; 32]);
        task.state = state;
        task.wait = wait.map(|wait| match wait {
            Wait::Operation { .. } => Wait::Operation {
                resolver: address.clone(),
                deadline: u64::MAX,
                max_result_bytes: 2048,
            },
            _ => Wait::Height { height: u64::MAX },
        });
        task.inbox = None;
        task.output = output;
        task.error = error;
        task.wait_sequence = u64::MAX;
        task.last_attempt = Some(u64::MAX);
        task.ready_at = u64::MAX;
        task.expires_at = u64::MAX;
        task.retain_until = u64::MAX;
        task.allocated_bytes = u64::MAX;
        task.charged = Uint128::MAX;
        task.reserved = Uint128::MAX;
        task.storage_fee = Uint128::MAX;
        task.status = ExecutionStatus::Completed;
        funding.task_id = u64::MAX;
        funding.contract = address.clone();
        funding.control = address.clone();
        funding.refund = address;
        funding.available = Uint128::MAX;
        funding.reserved = Uint128::MAX;
        funding.cumulative_execution_fees = Uint128::MAX;
        funding.storage_lease_paid = Uint128::MAX;
        funding.max_attempt_charge = Uint128::MAX;
        funding.max_gas_price = Decimal::MAX;
        funding.gas_per_poll = u64::MAX;
        funding.allocated_bytes = u64::MAX;
        funding.expires_at = u64::MAX;
        funding.status = TaskStatus::Paused {
            reason: PauseReason::InsufficientFunds,
            resume: ResumeState::Runnable,
        };
        // The actual task's private queue/expiry keys have fixed-width encoding.
        let wrappers = to_json_vec(&task).unwrap().len() + to_json_vec(&funding).unwrap().len();
        println!("actual {name} maximum-width wrappers bytes: {wrappers}");
        assert!(wrappers as u64 <= native_dca::contract::STATE_AND_HOST_ALLOWANCE);
        assert!(wrappers as u64 + 13_440 <= native_dca::contract::MIN_ALLOCATED_BYTES);
    }
}

#[test]
fn three_slices_have_minimum_interval_and_no_catchup() {
    // Catches omitted scheduling, registration-based catchup, and zero sleeps.
    let mut s = DcaSuite::new();
    s.create(s.owner.clone(), s.request()).unwrap();
    s.conservation(1);
    s.block(2).unwrap();
    s.conservation(1);
    assert_eq!(s.task(1).wait, Some(Wait::Height { height: 6 }));
    s.block(6).unwrap();
    s.conservation(1);
    assert_eq!(s.order(1).order.filled_count, 1);
    s.block(7).unwrap();
    s.conservation(1);
    assert_eq!(s.task(1).wait, Some(Wait::Height { height: 11 }));
    s.block(20).unwrap();
    s.conservation(1);
    assert_eq!(s.order(1).order.filled_count, 2);
    assert_eq!(s.block(20).unwrap().polls, 0);
    s.conservation(1);
    s.block(21).unwrap();
    s.conservation(1);
    assert_eq!(s.task(1).wait, Some(Wait::Height { height: 25 }));
    s.block(25).unwrap();
    s.conservation(1);
    let order = s.order(1).order;
    assert_eq!(
        (order.filled_count, order.spent, order.remaining),
        (3, Uint128::new(30), Uint128::zero())
    );
    assert_eq!(order.last_fill_height, Some(25));
    assert_eq!(
        s.app
            .wrap()
            .query_balance(&s.beneficiary, "uoutput")
            .unwrap()
            .amount,
        Uint128::new(45)
    );
    s.block(26).unwrap();
    s.conservation(1);
    assert_eq!(s.task(1).status, TaskStatus::Completed);
}

#[test]
fn all_filled_precedes_host_complete() {
    // Catches collapsing committed business settlement into host completion.
    let mut s = DcaSuite::new();
    let mut request = s.request();
    request.slice_count = 1;
    s.create(s.owner.clone(), request).unwrap();
    s.block(2).unwrap();
    s.block(6).unwrap();
    assert_eq!(s.order(1).order.filled_count, 1);
    assert!(s.task(1).inbox.is_some());
    assert_ne!(s.task(1).status, TaskStatus::Completed);
    s.block(7).unwrap();
    assert_eq!(s.task(1).status, TaskStatus::Completed);
}
