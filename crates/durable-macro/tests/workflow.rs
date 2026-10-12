use cosmwasm_std::{
    from_json,
    testing::{message_info, mock_dependencies, mock_env},
    to_json_binary, Addr,
};
use durable_macro::durable_workflow;
use durable_runtime::{
    self as runtime, Correlation, Deadline, Limits, Operation, Outcome, PreparedWait, Registry,
    Resolution, RuntimeError, RuntimeResult, Status, WaitError, Workflow as WorkflowTrait,
    WorkflowCtx,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
enum AppError {
    Wait(WaitError),
    Rejected,
}
impl From<WaitError> for AppError {
    fn from(value: WaitError) -> Self {
        Self::Wait(value)
    }
}

struct NumberWait;
impl Operation for NumberWait {
    const KIND: &'static str = "number";
    type Request = u32;
    type Output = u32;
    fn prepare(
        ctx: &mut WorkflowCtx<'_>,
        _: Correlation,
        request: u32,
    ) -> RuntimeResult<PreparedWait> {
        if request == 99 {
            return Err(RuntimeError::Validation("prepare rejected".into()));
        }
        Ok(PreparedWait {
            operation: Self::KIND.into(),
            request: to_json_binary(&request)?,
            resolver: ctx.env().contract.address.clone(),
            deadline: Deadline::Height(ctx.env().block.height + 5),
            messages: vec![],
        })
    }
}

fn ensure(value: u32) -> Result<u32, AppError> {
    if value == 0 {
        Err(AppError::Rejected)
    } else {
        Ok(value)
    }
}

fn environment() -> cosmwasm_std::Env {
    let mut env = mock_env();
    env.contract.address = cosmwasm_std::testing::MockApi::default().addr_make("resolver");
    env
}

#[durable_workflow(kind = "arithmetic", version = 1)]
async fn arithmetic(ctx: WorkflowCtx, input: u32) -> Result<u32, AppError> {
    let mut total: u32 = ensure(input)?;
    let first: u32 = ctx.wait::<NumberWait>(total).checkpoint(total).await?;
    total += first;
    let second: Result<u32, WaitError> = ctx.wait::<NumberWait>(total).checkpoint(total).await;
    let second: u32 = second?;
    Ok(total + second)
}

#[durable_workflow(kind = "empty", version = 3)]
async fn empty(ctx: WorkflowCtx, input: u32) -> Result<u32, AppError> {
    let value: u32 = ctx.wait::<NumberWait>(input).checkpoint().await?;
    Ok(value)
}

#[durable_workflow(kind = "shadowed", version = 1)]
async fn shadowed(ctx: WorkflowCtx, count: String) -> Result<u32, AppError> {
    let count: u32 = ctx
        .wait::<NumberWait>(count.len() as u32)
        .checkpoint(count)
        .await?;
    let value: u32 = ctx.wait::<NumberWait>(count).checkpoint(count).await?;
    Ok(value + count)
}

fn recover(result: Result<u32, WaitError>) -> u32 {
    result.unwrap_or(100)
}

#[durable_workflow(kind = "parent", version = 1)]
async fn parent(ctx: WorkflowCtx, input: u32) -> Result<u32, AppError> {
    let mut total: u32 = 10;
    total += 1;
    let child: u32 = ctx
        .call::<arithmetic::Workflow>(arithmetic::Input { input })
        .checkpoint(total)
        .await?;
    total += child;
    let own: u32 = ctx.wait::<NumberWait>(total).checkpoint(total).await?;
    Ok(total + own)
}

#[durable_workflow(kind = "immediate", version = 1)]
async fn immediate(ctx: WorkflowCtx, input: u32) -> Result<u32, AppError> {
    ensure(input)
}

#[durable_workflow(kind = "many_helpers", version = 1)]
async fn many_helpers(ctx: WorkflowCtx, limit: u32) -> Result<u32, AppError> {
    let mut total: u32 = 0;
    while total < limit {
        let next: u32 = ctx
            .call::<immediate::Workflow>(immediate::Input { input: total + 1 })
            .checkpoint(total, limit)
            .await?;
        total = next;
    }
    Ok(total)
}

#[test]
fn child_two_waits_then_parent_wait_share_monotonic_host_sequence() {
    let mut deps = mock_dependencies();
    let env = environment();
    let limits = Limits::default();
    let registry = Registry::new(vec![runtime::registration::<parent::Workflow>()]).unwrap();
    let (id, _) = runtime::start::<parent::Workflow>(
        deps.as_mut(),
        env.clone(),
        message_info(&Addr::unchecked("creator"), &[]),
        parent::Input { input: 4 },
        &limits,
    )
    .unwrap();
    for (sequence, request, answer) in [(1, 4, 2u32), (2, 6, 3), (3, 20, 5)] {
        match runtime::load_instance(&deps.storage, id).unwrap().status {
            Status::Waiting { state, wait } => {
                assert_eq!(from_json::<u32>(&wait.request).unwrap(), request);
                assert_eq!(parent::Workflow::operation(&state).unwrap(), "number");
                parent::Workflow::validate(
                    &state,
                    &Resolution::Outcome(Outcome::Success(to_json_binary(&answer).unwrap())),
                )
                .unwrap();
            }
            other => panic!("unexpected {other:?}"),
        }
        runtime::resume(
            deps.as_mut(),
            env.clone(),
            message_info(&env.contract.address, &[]),
            id,
            sequence,
            Outcome::Success(to_json_binary(&answer).unwrap()),
            &registry,
            &limits,
        )
        .unwrap();
    }
    match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Completed { output } => assert_eq!(from_json::<u32>(output).unwrap(), 25),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn immediate_helpers_loop_without_wait_or_recursive_parent_poll() {
    let mut deps = mock_dependencies();
    let (id, _) = runtime::start::<many_helpers::Workflow>(
        deps.as_mut(),
        environment(),
        message_info(&Addr::unchecked("creator"), &[]),
        many_helpers::Input { limit: 100_000 },
        &Limits::default(),
    )
    .unwrap();
    match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Completed { output } => assert_eq!(from_json::<u32>(output).unwrap(), 100_000),
        other => panic!("unexpected {other:?}"),
    }
}

#[durable_workflow(kind = "nested", version = 1)]
async fn nested(ctx: WorkflowCtx, input: u32) -> Result<u32, AppError> {
    let result: Result<u32, AppError> = ctx
        .call::<parent::Workflow>(parent::Input { input })
        .checkpoint()
        .await;
    Ok(result.unwrap_or(77))
}

#[test]
fn nested_child_errors_can_be_handled_or_propagated_and_runtime_errors_escape() {
    let mut deps = mock_dependencies();
    let limits = Limits::default();
    let info = message_info(&Addr::unchecked("creator"), &[]);
    let (id, _) = runtime::start::<nested::Workflow>(
        deps.as_mut(),
        environment(),
        info.clone(),
        nested::Input { input: 0 },
        &limits,
    )
    .unwrap();
    match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Completed { output } => assert_eq!(from_json::<u32>(output).unwrap(), 77),
        other => panic!("unexpected {other:?}"),
    }
    let (id, _) = runtime::start::<parent::Workflow>(
        deps.as_mut(),
        environment(),
        info.clone(),
        parent::Input { input: 0 },
        &limits,
    )
    .unwrap();
    match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Failed { error } => {
            assert_eq!(from_json::<AppError>(error).unwrap(), AppError::Rejected)
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(matches!(
        runtime::start::<nested::Workflow>(
            deps.as_mut(),
            environment(),
            info,
            nested::Input { input: 99 },
            &limits
        ),
        Err(RuntimeError::Validation(_))
    ));
}

#[test]
fn nested_waits_validate_pinned_identity_and_malformed_child_state() {
    let mut deps = mock_dependencies();
    let env = environment();
    let limits = Limits::default();
    let registry = Registry::new(vec![runtime::registration::<nested::Workflow>()]).unwrap();
    let (id, _) = runtime::start::<nested::Workflow>(
        deps.as_mut(),
        env.clone(),
        message_info(&Addr::unchecked("creator"), &[]),
        nested::Input { input: 4 },
        &limits,
    )
    .unwrap();
    let state = match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Waiting { state, .. } => state,
        other => panic!("{other:?}"),
    };
    let valid = Resolution::Outcome(Outcome::Success(to_json_binary(&2u32).unwrap()));
    nested::Workflow::validate(&state, &valid).unwrap();
    let json = String::from_utf8(state.to_vec()).unwrap();
    for invalid in [
        json.replace("\"parent\"", "\"other\""),
        json.replace("\"version\":1", "\"version\":2"),
        json.replace("\"state\":\"", "\"state\":\"AAAA"),
    ] {
        let invalid = cosmwasm_std::Binary::from(invalid.as_bytes());
        assert!(nested::Workflow::validate(&invalid, &valid).is_err());
        assert!(nested::Workflow::operation(&invalid).is_err());
    }
    assert!(nested::Workflow::validate(
        &state,
        &Resolution::Outcome(Outcome::Success(to_json_binary(&"bad").unwrap()))
    )
    .is_err());
    for (sequence, answer) in [(1, 2u32), (2, 3), (3, 5)] {
        runtime::resume(
            deps.as_mut(),
            env.clone(),
            message_info(&env.contract.address, &[]),
            id,
            sequence,
            Outcome::Success(to_json_binary(&answer).unwrap()),
            &registry,
            &limits,
        )
        .unwrap();
    }
    match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Completed { output } => assert_eq!(from_json::<u32>(output).unwrap(), 25),
        other => panic!("{other:?}"),
    }
}

struct MalformedChild;
impl WorkflowTrait for MalformedChild {
    const KIND: &'static str = "malformed";
    const VERSION: u32 = 1;
    type Input = bool;
    fn start(
        _: &mut WorkflowCtx<'_>,
        input: cosmwasm_std::Binary,
    ) -> RuntimeResult<runtime::Transition> {
        let bad = to_json_binary(&"not a typed result")?;
        if from_json::<bool>(input)? {
            Ok(runtime::Transition::Completed(bad))
        } else {
            Ok(runtime::Transition::Failed(bad))
        }
    }
    fn validate(_: &cosmwasm_std::Binary, _: &Resolution) -> RuntimeResult<()> {
        Ok(())
    }
    fn operation(_: &cosmwasm_std::Binary) -> RuntimeResult<&'static str> {
        Ok("unused")
    }
    fn resume(
        ctx: &mut WorkflowCtx<'_>,
        state: cosmwasm_std::Binary,
        _: Resolution,
    ) -> RuntimeResult<runtime::Transition> {
        Self::start(ctx, state)
    }
}
impl runtime::TypedWorkflow for MalformedChild {
    type Output = u32;
    type Error = AppError;
    const COMPOSITION_DEPTH: usize = 0;
}
fn mark_effect(ctx: &mut WorkflowCtx<'_>) {
    ctx.storage().set(b"parent-effect", b"ran");
}
#[durable_workflow(kind = "malformed_parent", version = 1)]
async fn malformed_parent(ctx: WorkflowCtx, success: bool) -> Result<u32, AppError> {
    let value: Result<u32, AppError> = ctx.call::<MalformedChild>(success).checkpoint().await;
    mark_effect(ctx);
    Ok(value.unwrap_or(77))
}

#[test]
fn malformed_typed_child_terminal_results_never_reach_parent_effects() {
    for success in [true, false] {
        let mut deps = mock_dependencies();
        assert!(runtime::start::<malformed_parent::Workflow>(
            deps.as_mut(),
            environment(),
            message_info(&Addr::unchecked("creator"), &[]),
            malformed_parent::Input { success },
            &Limits::default()
        )
        .is_err());
        assert_eq!(
            cosmwasm_std::Storage::get(&deps.storage, b"parent-effect"),
            None
        );
    }
}

#[test]
fn suspended_child_failure_is_handled_and_preparation_failure_is_runtime_error() {
    let mut deps = mock_dependencies();
    let env = environment();
    let limits = Limits::default();
    let registry = Registry::new(vec![runtime::registration::<nested::Workflow>()]).unwrap();
    let (id, _) = runtime::start::<nested::Workflow>(
        deps.as_mut(),
        env.clone(),
        message_info(&Addr::unchecked("creator"), &[]),
        nested::Input { input: 4 },
        &limits,
    )
    .unwrap();
    assert!(runtime::resume(
        deps.as_mut(),
        env.clone(),
        message_info(&env.contract.address, &[]),
        id,
        1,
        Outcome::Success(to_json_binary(&95u32).unwrap()),
        &registry,
        &limits
    )
    .is_err());
    // MockStorage has no transaction rollback; use a fresh instance after the
    // runtime error instead of pretending the failed invocation was rolled back.
    let (id, _) = runtime::start::<nested::Workflow>(
        deps.as_mut(),
        env.clone(),
        message_info(&Addr::unchecked("creator"), &[]),
        nested::Input { input: 4 },
        &limits,
    )
    .unwrap();
    runtime::resume(
        deps.as_mut(),
        env.clone(),
        message_info(&env.contract.address, &[]),
        id,
        1,
        Outcome::Error {
            code: "no".into(),
            message: "declined".into(),
        },
        &registry,
        &limits,
    )
    .unwrap();
    match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Completed { output } => assert_eq!(from_json::<u32>(output).unwrap(), 77),
        other => panic!("{other:?}"),
    }
}

#[durable_workflow(kind = "control_flow", version = 1)]
async fn control_flow(ctx: WorkflowCtx, limit: u32) -> Result<u32, AppError> {
    let mut total: u32 = 0;
    let mut iterations: u32 = 0;
    while iterations < limit {
        iterations += 1;
        if iterations == 1 {
            let first: u32 = ctx
                .wait::<NumberWait>(10)
                .checkpoint(limit, total, iterations)
                .await?;
            total += first;
        } else {
            let later: u32 = ctx
                .wait::<NumberWait>(total)
                .checkpoint(limit, total, iterations)
                .await?;
            if later == 0 {
                continue;
            }
            total += later;
            if total > 20 {
                break;
            }
        }
    }
    let final_value: u32 = ctx.wait::<NumberWait>(total).checkpoint(total).await?;
    Ok(total + final_value)
}

#[test]
fn branches_loops_break_continue_and_repeated_state_tags_resume_without_replay() {
    let mut deps = mock_dependencies();
    let env = environment();
    let limits = Limits::default();
    let registry = Registry::new(vec![runtime::registration::<control_flow::Workflow>()]).unwrap();
    let (id, _) = runtime::start::<control_flow::Workflow>(
        deps.as_mut(),
        env.clone(),
        message_info(&Addr::unchecked("creator"), &[]),
        control_flow::Input { limit: 8 },
        &limits,
    )
    .unwrap();
    for (sequence, expected_request, answer, expected_tag, expected_iterations) in [
        (1, 10, 3, "Waiting1", Some(1)),
        (2, 3, 0, "Waiting2", Some(2)),
        (3, 3, 7, "Waiting2", Some(3)),
        (4, 10, 15, "Waiting2", Some(4)),
        (5, 25, 2, "Waiting3", None),
    ] {
        match runtime::load_instance(&deps.storage, id).unwrap().status {
            Status::Waiting { state, wait } => {
                assert_eq!(from_json::<u32>(&wait.request).unwrap(), expected_request);
                let json = String::from_utf8(state.to_vec()).unwrap();
                assert!(json.contains(expected_tag), "{json}");
                if let Some(iterations) = expected_iterations {
                    assert!(
                        json.contains(&format!("\"iterations\":{iterations}")),
                        "{json}"
                    );
                }
                control_flow::Workflow::validate(
                    &state,
                    &Resolution::Outcome(Outcome::Success(to_json_binary(&answer).unwrap())),
                )
                .unwrap();
                assert_eq!(
                    control_flow::Workflow::operation(&state).unwrap(),
                    NumberWait::KIND
                );
            }
            other => panic!("unexpected {other:?}"),
        }
        runtime::resume(
            deps.as_mut(),
            env.clone(),
            message_info(&env.contract.address, &[]),
            id,
            sequence,
            Outcome::Success(to_json_binary(&answer).unwrap()),
            &registry,
            &limits,
        )
        .unwrap();
    }
    match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Completed { output } => assert_eq!(from_json::<u32>(&output).unwrap(), 27),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn zero_iteration_loop_skips_branch_waits_and_propagates_failure() {
    let mut deps = mock_dependencies();
    let env = environment();
    let limits = Limits::default();
    let registry = Registry::new(vec![runtime::registration::<control_flow::Workflow>()]).unwrap();
    let (id, _) = runtime::start::<control_flow::Workflow>(
        deps.as_mut(),
        env.clone(),
        message_info(&Addr::unchecked("creator"), &[]),
        control_flow::Input { limit: 0 },
        &limits,
    )
    .unwrap();
    match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Waiting { state, wait } => {
            assert_eq!(from_json::<u32>(&wait.request).unwrap(), 0);
            assert_eq!(
                String::from_utf8(state.to_vec()).unwrap(),
                r#"{"Waiting3":{"total":0}}"#
            );
        }
        other => panic!("unexpected {other:?}"),
    }
    runtime::resume(
        deps.as_mut(),
        env.clone(),
        message_info(&env.contract.address, &[]),
        id,
        1,
        Outcome::Error {
            code: "failed".into(),
            message: "remote".into(),
        },
        &registry,
        &limits,
    )
    .unwrap();
    match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Failed { error } => assert_eq!(
            from_json::<AppError>(&error).unwrap(),
            AppError::Wait(WaitError::Remote {
                code: "failed".into(),
                message: "remote".into()
            })
        ),
        other => panic!("unexpected {other:?}"),
    }
}

#[durable_workflow(kind = "recovery", version = 1)]
async fn recovery(ctx: WorkflowCtx) -> Result<u32, AppError> {
    let result: Result<u32, WaitError> = ctx.wait::<NumberWait>(1).checkpoint().await;
    Ok(recover(result))
}

#[test]
fn shadowed_binding_uses_new_type_at_next_checkpoint() {
    let mut deps = mock_dependencies();
    let env = environment();
    let limits = Limits::default();
    let registry = Registry::new(vec![runtime::registration::<shadowed::Workflow>()]).unwrap();
    let (id, _) = runtime::start::<shadowed::Workflow>(
        deps.as_mut(),
        env.clone(),
        message_info(&Addr::unchecked("creator"), &[]),
        shadowed::Input {
            count: "old string".into(),
        },
        &limits,
    )
    .unwrap();
    runtime::resume(
        deps.as_mut(),
        env.clone(),
        message_info(&env.contract.address, &[]),
        id,
        1,
        Outcome::Success(to_json_binary(&4u32).unwrap()),
        &registry,
        &limits,
    )
    .unwrap();
    let saved = runtime::load_instance(&deps.storage, id).unwrap();
    match saved.status {
        Status::Waiting { state, .. } => assert_eq!(
            String::from_utf8(state.to_vec()).unwrap(),
            r#"{"Waiting2":{"count":4}}"#
        ),
        other => panic!("unexpected {other:?}"),
    }
    let resolver = env.contract.address.clone();
    runtime::resume(
        deps.as_mut(),
        env,
        message_info(&resolver, &[]),
        id,
        2,
        Outcome::Success(to_json_binary(&3u32).unwrap()),
        &registry,
        &limits,
    )
    .unwrap();
    match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Completed { output } => assert_eq!(from_json::<u32>(&output).unwrap(), 7),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn unpropagated_remote_error_can_recover_to_success() {
    let mut deps = mock_dependencies();
    let env = environment();
    let limits = Limits::default();
    let registry = Registry::new(vec![runtime::registration::<recovery::Workflow>()]).unwrap();
    let (id, _) = runtime::start::<recovery::Workflow>(
        deps.as_mut(),
        env.clone(),
        message_info(&Addr::unchecked("creator"), &[]),
        recovery::Input {},
        &limits,
    )
    .unwrap();
    let resolver = env.contract.address.clone();
    runtime::resume(
        deps.as_mut(),
        env,
        message_info(&resolver, &[]),
        id,
        1,
        Outcome::Error {
            code: "failed".into(),
            message: "recover".into(),
        },
        &registry,
        &limits,
    )
    .unwrap();
    match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Completed { output } => assert_eq!(from_json::<u32>(&output).unwrap(), 100),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn two_waits_round_trip_mutable_checkpoint_and_unpropagated_result() {
    let mut deps = mock_dependencies();
    let env = environment();
    let limits = Limits::default();
    let registry = Registry::new(vec![runtime::registration::<arithmetic::Workflow>()]).unwrap();
    let (id, _) = runtime::start::<arithmetic::Workflow>(
        deps.as_mut(),
        env.clone(),
        message_info(&Addr::unchecked("creator"), &[]),
        arithmetic::Input { input: 4 },
        &limits,
    )
    .unwrap();
    let first = runtime::load_instance(&deps.storage, id).unwrap();
    let state = match first.status {
        Status::Waiting { state, wait } => {
            assert_eq!(from_json::<u32>(&wait.request).unwrap(), 4);
            state
        }
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(arithmetic::Workflow::operation(&state).unwrap(), "number");
    arithmetic::Workflow::validate(
        &state,
        &Resolution::Outcome(Outcome::Success(to_json_binary(&2u32).unwrap())),
    )
    .unwrap();
    runtime::resume(
        deps.as_mut(),
        env.clone(),
        message_info(&env.contract.address, &[]),
        id,
        1,
        Outcome::Success(to_json_binary(&2u32).unwrap()),
        &registry,
        &limits,
    )
    .unwrap();
    let second = runtime::load_instance(&deps.storage, id).unwrap();
    match second.status {
        Status::Waiting { state, wait } => {
            assert_eq!(from_json::<u32>(&wait.request).unwrap(), 6);
            assert!(!state.is_empty());
        }
        other => panic!("unexpected {other:?}"),
    }
    let resolver = env.contract.address.clone();
    runtime::resume(
        deps.as_mut(),
        env,
        message_info(&resolver, &[]),
        id,
        2,
        Outcome::Success(to_json_binary(&3u32).unwrap()),
        &registry,
        &limits,
    )
    .unwrap();
    let terminal = runtime::load_instance(&deps.storage, id).unwrap();
    match terminal.status {
        Status::Completed { output } => assert_eq!(from_json::<u32>(&output).unwrap(), 9),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn empty_checkpoint_can_resume_with_only_operation_output() {
    let mut deps = mock_dependencies();
    let env = environment();
    let limits = Limits::default();
    let registry = Registry::new(vec![runtime::registration::<empty::Workflow>()]).unwrap();
    let (id, _) = runtime::start::<empty::Workflow>(
        deps.as_mut(),
        env.clone(),
        message_info(&Addr::unchecked("creator"), &[]),
        empty::Input { input: 1 },
        &limits,
    )
    .unwrap();
    let resolver = env.contract.address.clone();
    runtime::resume(
        deps.as_mut(),
        env,
        message_info(&resolver, &[]),
        id,
        1,
        Outcome::Success(to_json_binary(&7u32).unwrap()),
        &registry,
        &limits,
    )
    .unwrap();
    match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Completed { output } => assert_eq!(from_json::<u32>(&output).unwrap(), 7),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn ordinary_question_mark_is_an_application_failure() {
    let mut deps = mock_dependencies();
    let (id, _) = runtime::start::<arithmetic::Workflow>(
        deps.as_mut(),
        mock_env(),
        message_info(&Addr::unchecked("creator"), &[]),
        arithmetic::Input { input: 0 },
        &Limits::default(),
    )
    .unwrap();
    match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Failed { error } => {
            assert_eq!(from_json::<AppError>(&error).unwrap(), AppError::Rejected)
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn wait_error_after_resume_is_an_application_failure() {
    let mut deps = mock_dependencies();
    let env = environment();
    let limits = Limits::default();
    let registry = Registry::new(vec![runtime::registration::<arithmetic::Workflow>()]).unwrap();
    let (id, _) = runtime::start::<arithmetic::Workflow>(
        deps.as_mut(),
        env.clone(),
        message_info(&Addr::unchecked("creator"), &[]),
        arithmetic::Input { input: 1 },
        &limits,
    )
    .unwrap();
    let resolver = env.contract.address.clone();
    runtime::resume(
        deps.as_mut(),
        env,
        message_info(&resolver, &[]),
        id,
        1,
        Outcome::Error {
            code: "declined".into(),
            message: "no".into(),
        },
        &registry,
        &limits,
    )
    .unwrap();
    match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Failed { error } => assert_eq!(
            from_json::<AppError>(&error).unwrap(),
            AppError::Wait(WaitError::Remote {
                code: "declined".into(),
                message: "no".into()
            })
        ),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn preparation_and_decoder_errors_remain_runtime_errors() {
    let mut deps = mock_dependencies();
    let env = environment();
    assert!(runtime::start::<arithmetic::Workflow>(
        deps.as_mut(),
        env.clone(),
        message_info(&Addr::unchecked("creator"), &[]),
        arithmetic::Input { input: 99 },
        &Limits::default()
    )
    .is_err());
    let (id, _) = runtime::start::<arithmetic::Workflow>(
        deps.as_mut(),
        env,
        message_info(&Addr::unchecked("creator"), &[]),
        arithmetic::Input { input: 1 },
        &Limits::default(),
    )
    .unwrap();
    match runtime::load_instance(&deps.storage, id).unwrap().status {
        Status::Waiting { state, .. } => assert!(arithmetic::Workflow::validate(
            &state,
            &Resolution::Outcome(Outcome::Success(to_json_binary(&"wrong type").unwrap()))
        )
        .is_err()),
        other => panic!("unexpected {other:?}"),
    }
}
