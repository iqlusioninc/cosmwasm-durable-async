use cosmwasm_std::{
    from_json,
    testing::{message_info, mock_dependencies, mock_env},
    to_json_binary, Binary, Env, Storage,
};
use durable_runtime::{
    active_count, assert_supported_versions, decode_resolution, expire, load_instance,
    migrate_waiting, registration, resume, start, Correlation, Deadline, Limits, Operation,
    Outcome, PreparedWait, Registry, Resolution, RuntimeError, RuntimeResult, Status, Transition,
    WaitError, Workflow, WorkflowCtx,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Input {
    value: u64,
    time: bool,
}

#[derive(Serialize, Deserialize)]
struct State {
    phase: u8,
    value: u64,
    time: bool,
}

#[derive(Serialize, Deserialize)]
struct Request {
    time: bool,
}

struct Number;
impl Operation for Number {
    const KIND: &'static str = "number";
    type Request = Request;
    type Output = u64;
    fn prepare(
        ctx: &mut WorkflowCtx<'_>,
        key: Correlation,
        request: Request,
    ) -> RuntimeResult<PreparedWait> {
        assert_eq!(key, ctx.correlation());
        let _fresh_querier: &cosmwasm_std::QuerierWrapper<'_> = ctx.querier();
        Ok(PreparedWait {
            // The runtime must normalize these adapter-controlled fields.
            operation: "forged-operation".into(),
            request: Binary::from(b"forged-request".as_slice()),
            resolver: cosmwasm_std::testing::MockApi::default().addr_make("resolver"),
            deadline: if request.time {
                Deadline::Time(ctx.env().block.time.seconds() + 10)
            } else {
                Deadline::Height(ctx.env().block.height + 10)
            },
            messages: vec![],
        })
    }
}

struct TwoStep;
impl Workflow for TwoStep {
    const KIND: &'static str = "two-step";
    const VERSION: u32 = 1;
    type Input = Input;
    fn start(ctx: &mut WorkflowCtx<'_>, input: Binary) -> RuntimeResult<Transition> {
        let input: Input = from_json(input)?;
        let wait = ctx.prepare::<Number>(Request { time: input.time })?;
        Ok(Transition::Wait {
            state: to_json_binary(&State {
                phase: 1,
                value: input.value,
                time: input.time,
            })?,
            wait,
        })
    }
    fn validate(state: &Binary, resolution: &Resolution) -> RuntimeResult<()> {
        let state: State = from_json(state)?;
        if !(1..=2).contains(&state.phase) {
            return Err(RuntimeError::Validation("invalid phase".into()));
        }
        let _ = decode_resolution::<Number>(resolution.clone())?;
        Ok(())
    }
    fn operation(state: &Binary) -> RuntimeResult<&'static str> {
        let _: State = from_json(state)?;
        Ok(Number::KIND)
    }
    fn resume(
        ctx: &mut WorkflowCtx<'_>,
        state: Binary,
        resolution: Resolution,
    ) -> RuntimeResult<Transition> {
        let state: State = from_json(state)?;
        ctx.storage().set(b"application/ran", b"yes");
        let number = match decode_resolution::<Number>(resolution)? {
            Ok(value) => value,
            Err(error) => return Ok(Transition::Failed(to_json_binary(&error)?)),
        };
        if state.phase == 1 {
            let wait = ctx.prepare::<Number>(Request { time: state.time })?;
            Ok(Transition::Wait {
                state: to_json_binary(&State {
                    phase: 2,
                    value: state.value + number,
                    time: state.time,
                })?,
                wait,
            })
        } else {
            Ok(Transition::Completed(to_json_binary(
                &(state.value + number),
            )?))
        }
    }
}

struct V2;
impl Workflow for V2 {
    const KIND: &'static str = TwoStep::KIND;
    const VERSION: u32 = 2;
    type Input = Input;
    fn start(ctx: &mut WorkflowCtx<'_>, input: Binary) -> RuntimeResult<Transition> {
        TwoStep::start(ctx, input)
    }
    fn validate(state: &Binary, resolution: &Resolution) -> RuntimeResult<()> {
        TwoStep::validate(state, resolution)
    }
    fn operation(state: &Binary) -> RuntimeResult<&'static str> {
        TwoStep::operation(state)
    }
    fn resume(
        ctx: &mut WorkflowCtx<'_>,
        state: Binary,
        resolution: Resolution,
    ) -> RuntimeResult<Transition> {
        TwoStep::resume(ctx, state, resolution)
    }
}

struct DifferentOperation;
impl Workflow for DifferentOperation {
    const KIND: &'static str = TwoStep::KIND;
    const VERSION: u32 = 3;
    type Input = Input;
    fn start(ctx: &mut WorkflowCtx<'_>, input: Binary) -> RuntimeResult<Transition> {
        TwoStep::start(ctx, input)
    }
    fn validate(state: &Binary, resolution: &Resolution) -> RuntimeResult<()> {
        TwoStep::validate(state, resolution)
    }
    fn operation(_: &Binary) -> RuntimeResult<&'static str> {
        Ok("different")
    }
    fn resume(
        ctx: &mut WorkflowCtx<'_>,
        state: Binary,
        resolution: Resolution,
    ) -> RuntimeResult<Transition> {
        TwoStep::resume(ctx, state, resolution)
    }
}

struct RuntimeFailure;
impl Workflow for RuntimeFailure {
    const KIND: &'static str = "runtime-failure";
    const VERSION: u32 = 1;
    type Input = Input;
    fn start(ctx: &mut WorkflowCtx<'_>, input: Binary) -> RuntimeResult<Transition> {
        TwoStep::start(ctx, input)
    }
    fn validate(state: &Binary, resolution: &Resolution) -> RuntimeResult<()> {
        TwoStep::validate(state, resolution)
    }
    fn operation(state: &Binary) -> RuntimeResult<&'static str> {
        TwoStep::operation(state)
    }
    fn resume(ctx: &mut WorkflowCtx<'_>, _: Binary, _: Resolution) -> RuntimeResult<Transition> {
        ctx.storage()
            .set(b"application/ran", b"before runtime error");
        Err(RuntimeError::Validation(
            "application invariant violated".into(),
        ))
    }
}

struct Immediate;
impl Workflow for Immediate {
    const KIND: &'static str = "immediate";
    const VERSION: u32 = 1;
    type Input = Binary;
    fn start(_: &mut WorkflowCtx<'_>, input: Binary) -> RuntimeResult<Transition> {
        let output: Binary = from_json(input)?;
        Ok(Transition::Completed(output))
    }
    fn validate(_: &Binary, _: &Resolution) -> RuntimeResult<()> {
        unreachable!()
    }
    fn operation(_: &Binary) -> RuntimeResult<&'static str> {
        unreachable!()
    }
    fn resume(_: &mut WorkflowCtx<'_>, _: Binary, _: Resolution) -> RuntimeResult<Transition> {
        unreachable!()
    }
}

struct Unchecked;
impl Workflow for Unchecked {
    const KIND: &'static str = "unchecked";
    const VERSION: u32 = 1;
    type Input = PreparedWait;
    fn start(_: &mut WorkflowCtx<'_>, input: Binary) -> RuntimeResult<Transition> {
        Ok(Transition::Wait {
            state: to_json_binary(&State {
                phase: 1,
                value: 0,
                time: false,
            })?,
            wait: from_json(input)?,
        })
    }
    fn validate(state: &Binary, resolution: &Resolution) -> RuntimeResult<()> {
        TwoStep::validate(state, resolution)
    }
    fn operation(state: &Binary) -> RuntimeResult<&'static str> {
        TwoStep::operation(state)
    }
    fn resume(
        ctx: &mut WorkflowCtx<'_>,
        state: Binary,
        resolution: Resolution,
    ) -> RuntimeResult<Transition> {
        TwoStep::resume(ctx, state, resolution)
    }
}

#[test]
fn forged_adapter_resolvers_and_already_reached_deadlines_are_rejected() {
    let env = mock_env();
    let valid = PreparedWait {
        operation: Number::KIND.into(),
        request: to_json_binary(&Request { time: false }).unwrap(),
        resolver: cosmwasm_std::testing::MockApi::default().addr_make("resolver"),
        deadline: Deadline::Height(env.block.height + 1),
        messages: vec![],
    };
    let cases = [
        PreparedWait {
            resolver: cosmwasm_std::Addr::unchecked("forged"),
            ..valid.clone()
        },
        PreparedWait {
            deadline: Deadline::Height(env.block.height),
            ..valid.clone()
        },
        PreparedWait {
            deadline: Deadline::Time(env.block.time.seconds()),
            ..valid.clone()
        },
        PreparedWait {
            operation: "forged".into(),
            ..valid
        },
    ];
    for wait in cases {
        let mut deps = mock_dependencies();
        let creator = deps.api.addr_make("creator");
        assert!(start::<Unchecked>(
            deps.as_mut(),
            env.clone(),
            message_info(&creator, &[]),
            wait,
            &Limits::default()
        )
        .is_err());
        assert!(load_instance(&deps.storage, 1).is_err());
    }
}

#[test]
fn serialized_checkpoint_request_and_callback_limits_allow_exact_equality() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let creator = deps.api.addr_make("creator");
    let resolver = deps.api.addr_make("resolver");
    let limits = Limits {
        max_checkpoint_bytes: to_json_binary(&State {
            phase: 1,
            value: 5,
            time: false,
        })
        .unwrap()
        .len(),
        max_request_bytes: to_json_binary(&Request { time: false }).unwrap().len(),
        max_callback_bytes: to_json_binary(&outcome(1)).unwrap().len(),
        ..Limits::default()
    };
    let (id, _) = start::<TwoStep>(
        deps.as_mut(),
        env.clone(),
        message_info(&creator, &[]),
        input(),
        &limits,
    )
    .unwrap();
    resume(
        deps.as_mut(),
        env.clone(),
        message_info(&resolver, &[]),
        id,
        1,
        outcome(1),
        &registry(),
        &limits,
    )
    .unwrap();
    let second = load_instance(&deps.storage, id).unwrap();
    assert!(resume(
        deps.as_mut(),
        env,
        message_info(&resolver, &[]),
        id,
        2,
        Outcome::Error {
            code: "large".into(),
            message: "oversized remote message".into()
        },
        &registry(),
        &limits
    )
    .is_err());
    assert_eq!(load_instance(&deps.storage, id).unwrap(), second);
}

#[test]
fn oversized_failed_record_returns_runtime_error_without_committed_terminal_record() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let creator = deps.api.addr_make("creator");
    let resolver = deps.api.addr_make("resolver");
    let limits = Limits {
        max_result_bytes: 1,
        ..Limits::default()
    };
    let (id, _) = start::<TwoStep>(
        deps.as_mut(),
        env.clone(),
        message_info(&creator, &[]),
        input(),
        &limits,
    )
    .unwrap();
    assert!(resume(
        deps.as_mut(),
        env,
        message_info(&resolver, &[]),
        id,
        1,
        Outcome::Error {
            code: "failure".into(),
            message: "large record".into()
        },
        &registry(),
        &limits
    )
    .is_err());
    // The host aborts this transaction. The nontransactional mock exposes only
    // the consumed marker, never an oversized terminal record.
    assert_eq!(
        load_instance(&deps.storage, id).unwrap().status,
        Status::Transitioning
    );
    assert_eq!(active_count(&deps.storage, TwoStep::KIND, 1).unwrap(), 1);
}

fn registry() -> Registry {
    Registry::new(vec![
        registration::<TwoStep>(),
        registration::<V2>(),
        registration::<DifferentOperation>(),
        registration::<RuntimeFailure>(),
    ])
    .unwrap()
}
fn input() -> Input {
    Input {
        value: 5,
        time: false,
    }
}
fn outcome(value: u64) -> Outcome {
    Outcome::Success(to_json_binary(&value).unwrap())
}
fn waiting(instance: &durable_runtime::Instance) -> (&Binary, &durable_runtime::WaitRecord) {
    match &instance.status {
        Status::Waiting { state, wait } => (state, wait),
        _ => panic!("expected wait"),
    }
}

#[test]
fn two_waits_round_trip_and_complete_with_monotonic_ids() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let creator = deps.api.addr_make("creator");
    let resolver = deps.api.addr_make("resolver");
    let limits = Limits::default();
    let (id, _) = start::<TwoStep>(
        deps.as_mut(),
        env.clone(),
        message_info(&creator, &[]),
        input(),
        &limits,
    )
    .unwrap();
    let first = load_instance(&deps.storage, id).unwrap();
    assert_eq!(first.creator, creator);
    assert_eq!(first.start_height, env.block.height);
    assert_eq!(waiting(&first).1.sequence, 1);
    assert_eq!(waiting(&first).1.operation, Number::KIND);
    assert!(from_json::<Request>(&waiting(&first).1.request).is_ok());
    assert_eq!(active_count(&deps.storage, TwoStep::KIND, 1).unwrap(), 1);
    // A newly constructed registry has no in-memory workflow state.
    resume(
        deps.as_mut(),
        env.clone(),
        message_info(&resolver, &[]),
        id,
        1,
        outcome(7),
        &registry(),
        &limits,
    )
    .unwrap();
    let second = load_instance(&deps.storage, id).unwrap();
    assert_eq!(waiting(&second).1.sequence, 2);
    assert_eq!(from_json::<State>(waiting(&second).0).unwrap().value, 12);
    resume(
        deps.as_mut(),
        env.clone(),
        message_info(&resolver, &[]),
        id,
        2,
        outcome(11),
        &registry(),
        &limits,
    )
    .unwrap();
    assert_eq!(
        load_instance(&deps.storage, id).unwrap().status,
        Status::Completed {
            output: to_json_binary(&23u64).unwrap()
        }
    );
    assert_eq!(active_count(&deps.storage, TwoStep::KIND, 1).unwrap(), 0);
    let (next, _) = start::<TwoStep>(
        deps.as_mut(),
        env,
        message_info(&creator, &[]),
        input(),
        &limits,
    )
    .unwrap();
    assert_eq!(next, id + 1);
}

#[test]
fn invalid_callbacks_leave_wait_unchanged_before_application_execution() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let creator = deps.api.addr_make("creator");
    let resolver = deps.api.addr_make("resolver");
    let limits = Limits::default();
    let (id, _) = start::<TwoStep>(
        deps.as_mut(),
        env.clone(),
        message_info(&creator, &[]),
        input(),
        &limits,
    )
    .unwrap();
    let original = load_instance(&deps.storage, id).unwrap();
    let attempts = [
        (creator.clone(), 1, outcome(1)),
        (resolver.clone(), 0, outcome(1)),
        (resolver.clone(), 2, outcome(1)),
        (
            resolver.clone(),
            1,
            Outcome::Success(Binary::from(b"not-json".as_slice())),
        ),
    ];
    for (sender, sequence, payload) in attempts {
        assert!(resume(
            deps.as_mut(),
            env.clone(),
            message_info(&sender, &[]),
            id,
            sequence,
            payload,
            &registry(),
            &limits
        )
        .is_err());
        assert_eq!(load_instance(&deps.storage, id).unwrap(), original);
        assert!(deps.storage.get(b"application/ran").is_none());
    }
    let tiny = Limits {
        max_callback_bytes: 1,
        ..Limits::default()
    };
    assert!(resume(
        deps.as_mut(),
        env.clone(),
        message_info(&resolver, &[]),
        id,
        1,
        outcome(1),
        &registry(),
        &tiny
    )
    .is_err());
    assert_eq!(load_instance(&deps.storage, id).unwrap(), original);
    resume(
        deps.as_mut(),
        env.clone(),
        message_info(&resolver, &[]),
        id,
        1,
        outcome(1),
        &registry(),
        &limits,
    )
    .unwrap();
    let second = load_instance(&deps.storage, id).unwrap();
    assert!(resume(
        deps.as_mut(),
        env.clone(),
        message_info(&resolver, &[]),
        id,
        1,
        outcome(1),
        &registry(),
        &limits
    )
    .is_err());
    assert!(expire(
        deps.as_mut(),
        env.clone(),
        message_info(&creator, &[]),
        id,
        1,
        &registry(),
        &limits
    )
    .is_err());
    assert_eq!(load_instance(&deps.storage, id).unwrap(), second);
    resume(
        deps.as_mut(),
        env.clone(),
        message_info(&resolver, &[]),
        id,
        2,
        outcome(1),
        &registry(),
        &limits,
    )
    .unwrap();
    let terminal = load_instance(&deps.storage, id).unwrap();
    assert!(resume(
        deps.as_mut(),
        env,
        message_info(&resolver, &[]),
        id,
        2,
        outcome(1),
        &registry(),
        &limits
    )
    .is_err());
    assert_eq!(load_instance(&deps.storage, id).unwrap(), terminal);
}

#[test]
fn remote_error_is_a_queryable_application_failure() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let creator = deps.api.addr_make("creator");
    let resolver = deps.api.addr_make("resolver");
    let limits = Limits::default();
    let (id, _) = start::<TwoStep>(
        deps.as_mut(),
        env.clone(),
        message_info(&creator, &[]),
        input(),
        &limits,
    )
    .unwrap();
    resume(
        deps.as_mut(),
        env,
        message_info(&resolver, &[]),
        id,
        1,
        Outcome::Error {
            code: "denied".into(),
            message: "remote rejected".into(),
        },
        &registry(),
        &limits,
    )
    .unwrap();
    match load_instance(&deps.storage, id).unwrap().status {
        Status::Failed { error } => assert_eq!(
            from_json::<WaitError>(error).unwrap(),
            WaitError::Remote {
                code: "denied".into(),
                message: "remote rejected".into()
            }
        ),
        _ => panic!("expected durable failure"),
    }
    assert_eq!(deps.storage.get(b"application/ran"), Some(b"yes".to_vec()));
    assert_eq!(active_count(&deps.storage, TwoStep::KIND, 1).unwrap(), 0);
}

fn at_deadline(mut env: Env, time: bool, delta: u64) -> Env {
    if time {
        env.block.time = cosmwasm_std::Timestamp::from_seconds(env.block.time.seconds() + delta);
    } else {
        env.block.height += delta;
    }
    env
}

#[test]
fn height_and_time_deadlines_share_exact_boundary() {
    for time in [false, true] {
        let mut deps = mock_dependencies();
        let env = mock_env();
        let creator = deps.api.addr_make("creator");
        let resolver = deps.api.addr_make("resolver");
        let anyone = deps.api.addr_make("anyone");
        let limits = Limits::default();
        let (id, _) = start::<TwoStep>(
            deps.as_mut(),
            env.clone(),
            message_info(&creator, &[]),
            Input { value: 5, time },
            &limits,
        )
        .unwrap();
        let before = at_deadline(env.clone(), time, 9);
        assert!(expire(
            deps.as_mut(),
            before.clone(),
            message_info(&anyone, &[]),
            id,
            1,
            &registry(),
            &limits
        )
        .is_err());
        // A callback one tick before equality is accepted.
        resume(
            deps.as_mut(),
            before,
            message_info(&resolver, &[]),
            id,
            1,
            outcome(1),
            &registry(),
            &limits,
        )
        .unwrap();
        let (deadline_id, _) = start::<TwoStep>(
            deps.as_mut(),
            env.clone(),
            message_info(&creator, &[]),
            Input { value: 5, time },
            &limits,
        )
        .unwrap();
        let equal = at_deadline(env, time, 10);
        let original = load_instance(&deps.storage, deadline_id).unwrap();
        assert!(resume(
            deps.as_mut(),
            equal.clone(),
            message_info(&resolver, &[]),
            deadline_id,
            1,
            outcome(1),
            &registry(),
            &limits
        )
        .is_err());
        assert_eq!(load_instance(&deps.storage, deadline_id).unwrap(), original);
        expire(
            deps.as_mut(),
            equal.clone(),
            message_info(&anyone, &[]),
            deadline_id,
            1,
            &registry(),
            &limits,
        )
        .unwrap();
        match load_instance(&deps.storage, deadline_id).unwrap().status {
            Status::Failed { error } => {
                assert_eq!(from_json::<WaitError>(error).unwrap(), WaitError::Timeout)
            }
            _ => panic!("expected timeout failure"),
        }
        assert!(expire(
            deps.as_mut(),
            equal,
            message_info(&anyone, &[]),
            deadline_id,
            1,
            &registry(),
            &limits
        )
        .is_err());
    }
}

#[test]
fn limits_cover_global_active_checkpoint_request_and_terminal_output() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let creator = deps.api.addr_make("creator");
    let one = Limits {
        max_active: 1,
        ..Limits::default()
    };
    start::<TwoStep>(
        deps.as_mut(),
        env.clone(),
        message_info(&creator, &[]),
        input(),
        &one,
    )
    .unwrap();
    assert!(start::<V2>(
        deps.as_mut(),
        env.clone(),
        message_info(&creator, &[]),
        input(),
        &one
    )
    .is_err());
    for limits in [
        Limits {
            max_checkpoint_bytes: 1,
            ..Limits::default()
        },
        Limits {
            max_request_bytes: 1,
            ..Limits::default()
        },
    ] {
        let mut fresh = mock_dependencies();
        assert!(start::<TwoStep>(
            fresh.as_mut(),
            env.clone(),
            message_info(&creator, &[]),
            input(),
            &limits
        )
        .is_err());
    }
    let mut fresh = mock_dependencies();
    let limits = Limits {
        max_result_bytes: 3,
        ..Limits::default()
    };
    let (id, _) = start::<Immediate>(
        fresh.as_mut(),
        env.clone(),
        message_info(&creator, &[]),
        Binary::from(b"123".as_slice()),
        &limits,
    )
    .unwrap();
    assert_eq!(
        load_instance(&fresh.storage, id).unwrap().status,
        Status::Completed {
            output: Binary::from(b"123".as_slice())
        }
    );
    assert_eq!(active_count(&fresh.storage, Immediate::KIND, 1).unwrap(), 0);
    assert!(start::<Immediate>(
        fresh.as_mut(),
        env,
        message_info(&creator, &[]),
        Binary::from(b"1234".as_slice()),
        &limits
    )
    .is_err());
}

#[test]
fn registry_coverage_and_explicit_migration_preserve_live_wait() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let creator = deps.api.addr_make("creator");
    let resolver = deps.api.addr_make("resolver");
    let limits = Limits::default();
    let (id, _) = start::<TwoStep>(
        deps.as_mut(),
        env.clone(),
        message_info(&creator, &[]),
        input(),
        &limits,
    )
    .unwrap();
    let old = load_instance(&deps.storage, id).unwrap();
    let v2_only = Registry::new(vec![registration::<V2>()]).unwrap();
    assert!(assert_supported_versions(&deps.storage, &v2_only).is_err());
    assert!(resume(
        deps.as_mut(),
        env.clone(),
        message_info(&resolver, &[]),
        id,
        1,
        outcome(1),
        &v2_only,
        &limits
    )
    .is_err());
    assert_eq!(load_instance(&deps.storage, id).unwrap(), old);
    assert!(Registry::new(vec![registration::<V2>(), registration::<V2>()]).is_err());
    assert!(migrate_waiting(
        &mut deps.storage,
        id,
        3,
        waiting(&old).0.clone(),
        &registry(),
        &limits
    )
    .is_err());
    assert!(migrate_waiting(
        &mut deps.storage,
        id,
        2,
        Binary::from(b"invalid".as_slice()),
        &registry(),
        &limits
    )
    .is_err());
    assert_eq!(load_instance(&deps.storage, id).unwrap(), old);
    let state = to_json_binary(&State {
        phase: 1,
        value: 100,
        time: false,
    })
    .unwrap();
    let migrated = migrate_waiting(
        &mut deps.storage,
        id,
        2,
        state.clone(),
        &registry(),
        &limits,
    )
    .unwrap();
    assert_eq!(migrated.version, 2);
    assert_eq!(migrated.creator, old.creator);
    assert_eq!(migrated.start_height, old.start_height);
    assert_eq!(waiting(&migrated).1, waiting(&old).1);
    assert_eq!(waiting(&migrated).0, &state);
    assert_eq!(active_count(&deps.storage, TwoStep::KIND, 1).unwrap(), 0);
    assert_eq!(active_count(&deps.storage, TwoStep::KIND, 2).unwrap(), 1);
    assert_supported_versions(&deps.storage, &v2_only).unwrap();
    resume(
        deps.as_mut(),
        env.clone(),
        message_info(&resolver, &[]),
        id,
        1,
        outcome(1),
        &v2_only,
        &limits,
    )
    .unwrap();
    resume(
        deps.as_mut(),
        env,
        message_info(&resolver, &[]),
        id,
        2,
        outcome(1),
        &v2_only,
        &limits,
    )
    .unwrap();
    assert_eq!(
        load_instance(&deps.storage, id).unwrap().status,
        Status::Completed {
            output: to_json_binary(&102u64).unwrap()
        }
    );
    assert!(migrate_waiting(&mut deps.storage, id, 1, state, &registry(), &limits).is_err());
}

#[test]
fn runtime_errors_require_host_rollback_instead_of_mock_storage_rollback() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let creator = deps.api.addr_make("creator");
    let resolver = deps.api.addr_make("resolver");
    let limits = Limits::default();
    let (id, _) = start::<RuntimeFailure>(
        deps.as_mut(),
        env.clone(),
        message_info(&creator, &[]),
        input(),
        &limits,
    )
    .unwrap();
    assert!(resume(
        deps.as_mut(),
        env,
        message_info(&resolver, &[]),
        id,
        1,
        outcome(1),
        &registry(),
        &limits
    )
    .is_err());
    // MockStorage deliberately has no transaction overlay. The CosmWasm host must
    // roll these writes back when contract integration returns the runtime error.
    assert_eq!(
        load_instance(&deps.storage, id).unwrap().status,
        Status::Transitioning
    );
    assert_eq!(
        deps.storage.get(b"application/ran"),
        Some(b"before runtime error".to_vec())
    );
}

#[test]
fn final_sequence_allows_terminal_resolution_but_rejects_another_wait() {
    for (phase, timeout) in [(2, false), (2, true), (1, false)] {
        let mut deps = mock_dependencies();
        let mut env = mock_env();
        let creator = deps.api.addr_make("creator");
        let resolver = deps.api.addr_make("resolver");
        let limits = Limits::default();
        let (id, _) = start::<TwoStep>(
            deps.as_mut(),
            env.clone(),
            message_info(&creator, &[]),
            input(),
            &limits,
        )
        .unwrap();
        let mut instance = load_instance(&deps.storage, id).unwrap();
        if let Status::Waiting { state, wait } = &mut instance.status {
            *state = to_json_binary(&State {
                phase,
                value: 5,
                time: false,
            })
            .unwrap();
            wait.sequence = u64::MAX;
        }
        // Seed an otherwise valid persisted record at the monotonic-counter boundary.
        let mut key = b"durable-runtime/v1/instance/".to_vec();
        key.extend_from_slice(&id.to_be_bytes());
        deps.storage.set(&key, &to_json_binary(&instance).unwrap());
        let result = if timeout {
            env.block.height += 10;
            expire(
                deps.as_mut(),
                env,
                message_info(&creator, &[]),
                id,
                u64::MAX,
                &registry(),
                &limits,
            )
        } else {
            resume(
                deps.as_mut(),
                env,
                message_info(&resolver, &[]),
                id,
                u64::MAX,
                outcome(1),
                &registry(),
                &limits,
            )
        };
        if phase == 1 {
            assert!(matches!(result, Err(RuntimeError::CounterOverflow)));
        } else {
            result.unwrap();
            assert_eq!(active_count(&deps.storage, TwoStep::KIND, 1).unwrap(), 0);
            assert!(matches!(
                load_instance(&deps.storage, id).unwrap().status,
                Status::Completed { .. } | Status::Failed { .. }
            ));
        }
    }
}
