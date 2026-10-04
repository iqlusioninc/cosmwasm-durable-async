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
