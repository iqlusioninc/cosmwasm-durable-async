use std::collections::BTreeMap;

use cosmwasm_std::Binary;
use serde::{de::DeserializeOwned, Serialize};

use crate::{Resolution, RuntimeError, RuntimeResult, Transition, WorkflowCtx};

pub trait Workflow {
    const KIND: &'static str;
    const VERSION: u32;
    type Input: Serialize + DeserializeOwned;

    fn start(ctx: &mut WorkflowCtx<'_>, input: Binary) -> RuntimeResult<Transition>;
    /// Purely validates the continuation and typed resolution, without running
    /// application logic or mutating storage.
    fn validate(state: &Binary, resolution: &Resolution) -> RuntimeResult<()>;
    fn operation(state: &Binary) -> RuntimeResult<&'static str>;
    fn resume(
        ctx: &mut WorkflowCtx<'_>,
        state: Binary,
        resolution: Resolution,
    ) -> RuntimeResult<Transition>;
}

/// Typed child composition. Definitions are immutable: a change to any child's
/// encoding or meaning requires a new containing workflow version. Manual
/// implementations must report an honest, acyclic composition depth.
pub trait TypedWorkflow: Workflow {
    type Output: Serialize + DeserializeOwned;
    type Error: Serialize + DeserializeOwned;
    const COMPOSITION_DEPTH: usize;
}

/// Persisted child identity, pinned to the parent definition at its call site.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ChildContinuation {
    pub kind: String,
    pub version: u32,
    pub state: Binary,
}

impl ChildContinuation {
    pub fn new<W: TypedWorkflow>(state: Binary) -> Self {
        Self {
            kind: W::KIND.into(),
            version: W::VERSION,
            state,
        }
    }

    pub fn check<W: TypedWorkflow>(&self) -> RuntimeResult<()> {
        if self.kind != W::KIND || self.version != W::VERSION {
            return Err(RuntimeError::Validation(
                "durable child identity/version mismatch".into(),
            ));
        }
        Ok(())
    }
}

pub fn decode_child_result<W: TypedWorkflow>(
    transition: Transition,
) -> RuntimeResult<Result<W::Output, W::Error>> {
    match transition {
        Transition::Completed(value) => Ok(Ok(cosmwasm_std::from_json(value)?)),
        Transition::Failed(value) => Ok(Err(cosmwasm_std::from_json(value)?)),
        Transition::Wait { .. } => Err(RuntimeError::Validation("child is still waiting".into())),
    }
}

pub type StartHandler = for<'a> fn(&mut WorkflowCtx<'a>, Binary) -> RuntimeResult<Transition>;
pub type ValidateHandler = fn(&Binary, &Resolution) -> RuntimeResult<()>;
pub type OperationHandler = fn(&Binary) -> RuntimeResult<&'static str>;
pub type ResumeHandler =
    for<'a> fn(&mut WorkflowCtx<'a>, Binary, Resolution) -> RuntimeResult<Transition>;

#[derive(Clone, Copy, Debug)]
pub struct WorkflowRegistration {
    pub kind: &'static str,
    pub version: u32,
    pub start: StartHandler,
    pub validate: ValidateHandler,
    pub operation: OperationHandler,
    pub resume: ResumeHandler,
}

pub fn registration<W: Workflow>() -> WorkflowRegistration {
    WorkflowRegistration {
        kind: W::KIND,
        version: W::VERSION,
        start: W::start,
        validate: W::validate,
        operation: W::operation,
        resume: W::resume,
    }
}

#[derive(Clone, Debug)]
pub struct Registry {
    entries: BTreeMap<(String, u32), WorkflowRegistration>,
}

impl Registry {
    pub fn new(entries: Vec<WorkflowRegistration>) -> RuntimeResult<Self> {
        let mut registry = Self {
            entries: BTreeMap::new(),
        };
        for entry in entries {
            validate_identity(entry.kind, entry.version)?;
            let key = (entry.kind.into(), entry.version);
            if registry.entries.insert(key, entry).is_some() {
                return Err(RuntimeError::DuplicateRegistration {
                    kind: entry.kind.into(),
                    version: entry.version,
                });
            }
        }
        Ok(registry)
    }

    pub fn get(&self, kind: &str, version: u32) -> RuntimeResult<&WorkflowRegistration> {
        self.entries
            .get(&(kind.into(), version))
            .ok_or_else(|| RuntimeError::UnsupportedWorkflow {
                kind: kind.into(),
                version,
            })
    }
}

pub(crate) fn validate_identity(kind: &str, version: u32) -> RuntimeResult<()> {
    if kind.is_empty() {
        return Err(RuntimeError::InvalidKind);
    }
    if version == 0 {
        return Err(RuntimeError::InvalidVersion);
    }
    Ok(())
}
