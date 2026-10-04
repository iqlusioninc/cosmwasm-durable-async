use cosmwasm_std::{
    from_json, to_json_binary, Api, Binary, DepsMut, Env, MessageInfo, Response, Storage,
};
use serde::{Deserialize, Serialize};

use crate::{
    registry::validate_identity, Correlation, Instance, Limits, Outcome, Registry, Resolution,
    RuntimeError, RuntimeResult, Status, Transition, WaitRecord, Workflow, WorkflowCtx,
    WorkflowRegistration,
};

const META_KEY: &[u8] = b"durable-runtime/v1/meta";
const INSTANCE_PREFIX: &[u8] = b"durable-runtime/v1/instance/";

#[derive(Default, Serialize, Deserialize)]
struct Metadata {
    last_id: u64,
    total_active: u64,
    versions: Vec<ActiveVersion>,
}

#[derive(Serialize, Deserialize)]
struct ActiveVersion {
    kind: String,
    version: u32,
    count: u64,
}

fn metadata(storage: &dyn Storage) -> RuntimeResult<Metadata> {
    match storage.get(META_KEY) {
        Some(encoded) => Ok(from_json(encoded)?),
        None => Ok(Metadata::default()),
    }
}

fn instance_key(workflow_id: u64) -> Vec<u8> {
    let mut key = INSTANCE_PREFIX.to_vec();
    key.extend_from_slice(&workflow_id.to_be_bytes());
    key
}

fn save_instance(storage: &mut dyn Storage, instance: &Instance) -> RuntimeResult<()> {
    storage.set(
        &instance_key(instance.workflow_id),
        &to_json_binary(instance)?,
    );
    Ok(())
}

fn save_metadata(storage: &mut dyn Storage, metadata: &Metadata) -> RuntimeResult<()> {
    storage.set(META_KEY, &to_json_binary(metadata)?);
    Ok(())
}

fn increase_active(metadata: &mut Metadata, kind: &str, version: u32) -> RuntimeResult<()> {
    metadata.total_active = metadata
        .total_active
        .checked_add(1)
        .ok_or(RuntimeError::CounterOverflow)?;
    if let Some(entry) = metadata
        .versions
        .iter_mut()
        .find(|entry| entry.kind == kind && entry.version == version)
    {
        entry.count = entry
            .count
            .checked_add(1)
            .ok_or(RuntimeError::CounterOverflow)?;
    } else {
        metadata.versions.push(ActiveVersion {
            kind: kind.into(),
            version,
            count: 1,
        });
    }
    Ok(())
}

fn decrease_active(metadata: &mut Metadata, kind: &str, version: u32) -> RuntimeResult<()> {
    metadata.total_active = metadata
        .total_active
        .checked_sub(1)
        .ok_or_else(|| RuntimeError::Validation("active count underflow".into()))?;
    let entry = metadata
        .versions
        .iter_mut()
        .find(|entry| entry.kind == kind && entry.version == version)
        .ok_or_else(|| RuntimeError::Validation("missing active version count".into()))?;
    entry.count = entry
        .count
        .checked_sub(1)
        .ok_or_else(|| RuntimeError::Validation("active version count underflow".into()))?;
    Ok(())
}

fn check_size(limit: &'static str, actual: usize, max: usize) -> RuntimeResult<()> {
    if actual > max {
        return Err(RuntimeError::LimitExceeded { limit, max, actual });
    }
    Ok(())
}

pub fn load_instance(storage: &dyn Storage, workflow_id: u64) -> RuntimeResult<Instance> {
    let encoded = storage
        .get(&instance_key(workflow_id))
        .ok_or(RuntimeError::MissingInstance(workflow_id))?;
    Ok(from_json(encoded)?)
}

pub fn active_count(storage: &dyn Storage, kind: &str, version: u32) -> RuntimeResult<u64> {
    Ok(metadata(storage)?
        .versions
        .into_iter()
        .find(|entry| entry.kind == kind && entry.version == version)
        .map_or(0, |entry| entry.count))
}

/// Call from contract migration before accepting the new handler registry.
pub fn assert_supported_versions(storage: &dyn Storage, registry: &Registry) -> RuntimeResult<()> {
    for entry in metadata(storage)?.versions {
        if entry.count > 0 {
            registry.get(&entry.kind, entry.version)?;
        }
    }
    Ok(())
}

/// Executes one start segment. The contract controls start authorization.
/// On error, contract integration must abort the host transaction.
pub fn start<W: Workflow>(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    input: W::Input,
    limits: &Limits,
) -> RuntimeResult<(u64, Response)> {
    validate_identity(W::KIND, W::VERSION)?;
    let mut meta = metadata(deps.storage)?;
    if meta.total_active >= limits.max_active {
        return Err(RuntimeError::ActiveLimitReached(limits.max_active));
    }
    let id = meta
        .last_id
        .checked_add(1)
        .ok_or(RuntimeError::CounterOverflow)?;
    let input = to_json_binary(&input)?;
    meta.last_id = id;
    // Reserving before application execution prevents a reentrant segment from
    // observing an unallocated ID. Errors rely on host transaction rollback.
    save_metadata(deps.storage, &meta)?;
    let mut ctx = WorkflowCtx {
        storage: deps.storage,
        api: deps.api,
        querier: deps.querier,
        env: &env,
        info: &info,
        correlation: Correlation {
            workflow_id: id,
            wait_sequence: 1,
        },
    };
    let transition = W::start(&mut ctx, input)?;
    let instance = Instance {
        workflow_id: id,
        kind: W::KIND.into(),
        version: W::VERSION,
        creator: info.sender.clone(),
        start_height: env.block.height,
        status: Status::Transitioning,
    };
    let response = persist_transition(
        deps.storage,
        deps.api,
        &env,
        instance,
        1,
        transition,
        &crate::registration::<W>(),
        limits,
        false,
    )?;
    Ok((id, response))
}

/// Authenticated resolution before the saved deadline. Callback size is measured
/// on the JSON-encoded outcome envelope, including remote error strings.
#[allow(clippy::too_many_arguments)]
pub fn resume(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    workflow_id: u64,
    wait_sequence: u64,
    outcome: Outcome,
    registry: &Registry,
    limits: &Limits,
) -> RuntimeResult<Response> {
    resolve(
        deps,
        env,
        info,
        Correlation {
            workflow_id,
            wait_sequence,
        },
        Resolution::Outcome(outcome),
        registry,
        limits,
    )
}

/// Permissionless expiration at or after the saved deadline.
pub fn expire(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    workflow_id: u64,
    wait_sequence: u64,
    registry: &Registry,
    limits: &Limits,
) -> RuntimeResult<Response> {
    resolve(
        deps,
        env,
        info,
        Correlation {
            workflow_id,
            wait_sequence,
        },
        Resolution::Timeout,
        registry,
        limits,
    )
}

fn resolve(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    key: Correlation,
    resolution: Resolution,
    registry: &Registry,
    limits: &Limits,
) -> RuntimeResult<Response> {
    let mut instance = load_instance(deps.storage, key.workflow_id)?;
    let (state, wait) = match &instance.status {
        Status::Waiting { state, wait } => (state.clone(), wait.clone()),
        _ => return Err(RuntimeError::NotWaiting),
    };
    if wait.sequence != key.wait_sequence {
        return Err(RuntimeError::StaleWait {
            expected: wait.sequence,
            received: key.wait_sequence,
        });
    }
    match &resolution {
        Resolution::Outcome(outcome) => {
            if info.sender != wait.resolver {
                return Err(RuntimeError::Unauthorized);
            }
            if wait.deadline.reached(&env) {
                return Err(RuntimeError::DeadlineReached);
            }
            check_size(
                "callback bytes",
                to_json_binary(outcome)?.len(),
                limits.max_callback_bytes,
            )?;
        }
        Resolution::Timeout => {
            if !wait.deadline.reached(&env) {
                return Err(RuntimeError::DeadlineNotReached);
            }
        }
    }
    let handler = registry.get(&instance.kind, instance.version)?;
    if (handler.operation)(&state)? != wait.operation {
        return Err(RuntimeError::Validation(
            "saved operation does not match continuation".into(),
        ));
    }
    (handler.validate)(&state, &resolution)?;
    let next_sequence = wait.sequence.checked_add(1);
    instance.status = Status::Transitioning;
    save_instance(deps.storage, &instance)?;
    let mut ctx = WorkflowCtx {
        storage: deps.storage,
        api: deps.api,
        querier: deps.querier,
        env: &env,
        info: &info,
        // A terminal segment needs no fresh correlation. At the final sequence,
        // any preparation can only become observable if a new wait commits;
        // the check below rejects that transition before messages are returned.
        correlation: Correlation {
            workflow_id: key.workflow_id,
            wait_sequence: next_sequence.unwrap_or(u64::MAX),
        },
    };
    let transition = (handler.resume)(&mut ctx, state, resolution)?;
    if next_sequence.is_none() && matches!(transition, Transition::Wait { .. }) {
        return Err(RuntimeError::CounterOverflow);
    }
    persist_transition(
        deps.storage,
        deps.api,
        &env,
        instance,
        next_sequence.unwrap_or(u64::MAX),
        transition,
        handler,
        limits,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn persist_transition(
    storage: &mut dyn Storage,
    api: &dyn Api,
    env: &Env,
    mut instance: Instance,
    sequence: u64,
    transition: Transition,
    handler: &WorkflowRegistration,
    limits: &Limits,
    was_active: bool,
) -> RuntimeResult<Response> {
    let mut response = Response::new()
        .add_attribute("workflow_id", instance.workflow_id.to_string())
        .add_attribute("workflow_kind", &instance.kind)
        .add_attribute("workflow_version", instance.version.to_string());
    instance.status = match transition {
        Transition::Wait { state, wait } => {
            check_size("checkpoint bytes", state.len(), limits.max_checkpoint_bytes)?;
            check_size(
                "request bytes",
                wait.request.len(),
                limits.max_request_bytes,
            )?;
            let resolver = api.addr_validate(wait.resolver.as_str())?;
            if wait.deadline.reached(env) {
                return Err(RuntimeError::DeadlineReached);
            }
            if wait.operation.is_empty() {
                return Err(RuntimeError::InvalidKind);
            }
            if (handler.operation)(&state)? != wait.operation {
                return Err(RuntimeError::Validation(
                    "prepared operation does not match continuation".into(),
                ));
            }
            (handler.validate)(&state, &Resolution::Timeout)?;
            response = response
                .add_attribute("workflow_status", "waiting")
                .add_attribute("wait_sequence", sequence.to_string())
                .add_messages(wait.messages);
            Status::Waiting {
                state,
                wait: WaitRecord {
                    sequence,
                    operation: wait.operation,
                    request: wait.request,
                    resolver,
                    deadline: wait.deadline,
                },
            }
        }
        Transition::Completed(output) => {
            check_size("result bytes", output.len(), limits.max_result_bytes)?;
            response = response.add_attribute("workflow_status", "completed");
            Status::Completed { output }
        }
        Transition::Failed(error) => {
            check_size("result bytes", error.len(), limits.max_result_bytes)?;
            response = response.add_attribute("workflow_status", "failed");
            Status::Failed { error }
        }
    };
    let is_active = matches!(instance.status, Status::Waiting { .. });
    let mut meta = metadata(storage)?;
    match (was_active, is_active) {
        (false, true) => increase_active(&mut meta, &instance.kind, instance.version)?,
        (true, false) => decrease_active(&mut meta, &instance.kind, instance.version)?,
        _ => {}
    }
    save_instance(storage, &instance)?;
    save_metadata(storage, &meta)?;
    Ok(response)
}

/// Explicitly replaces a waiting continuation and definition version. Contract
/// migration authorization belongs to the caller. Correlation, request, resolver,
/// deadline, creator and start height are preserved. The target handler must
/// validate the checkpoint and select the same outstanding operation kind.
/// The timeout resolution supplies a payload-free schema validation path.
pub fn migrate_waiting(
    storage: &mut dyn Storage,
    workflow_id: u64,
    target_version: u32,
    state: Binary,
    registry: &Registry,
    limits: &Limits,
) -> RuntimeResult<Instance> {
    let mut instance = load_instance(storage, workflow_id)?;
    let wait = match &instance.status {
        Status::Waiting { wait, .. } => wait.clone(),
        _ => return Err(RuntimeError::NotWaiting),
    };
    check_size("checkpoint bytes", state.len(), limits.max_checkpoint_bytes)?;
    let target = registry.get(&instance.kind, target_version)?;
    if (target.operation)(&state)? != wait.operation {
        return Err(RuntimeError::Validation(
            "migration changes outstanding operation identity".into(),
        ));
    }
    (target.validate)(&state, &Resolution::Timeout)?;
    let mut meta = metadata(storage)?;
    if target_version != instance.version {
        decrease_active(&mut meta, &instance.kind, instance.version)?;
        increase_active(&mut meta, &instance.kind, target_version)?;
    }
    instance.version = target_version;
    instance.status = Status::Waiting { state, wait };
    save_instance(storage, &instance)?;
    save_metadata(storage, &meta)?;
    Ok(instance)
}
