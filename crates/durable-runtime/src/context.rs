use cosmwasm_std::{from_json, to_json_binary, Api, Env, MessageInfo, QuerierWrapper, Storage};
use serde::{de::DeserializeOwned, Serialize};

use crate::{
    Correlation, Outcome, PreparedWait, Resolution, RuntimeError, RuntimeResult, WaitError,
};

/// Fresh invocation capabilities, never part of a saved continuation.
pub struct WorkflowCtx<'a> {
    pub(crate) storage: &'a mut dyn Storage,
    pub(crate) api: &'a dyn Api,
    pub(crate) querier: QuerierWrapper<'a>,
    pub(crate) env: &'a Env,
    pub(crate) info: &'a MessageInfo,
    pub(crate) correlation: Correlation,
}

impl WorkflowCtx<'_> {
    pub fn storage(&mut self) -> &mut dyn Storage {
        self.storage
    }
    pub fn api(&self) -> &dyn Api {
        self.api
    }
    pub fn querier(&self) -> &QuerierWrapper<'_> {
        &self.querier
    }
    pub fn env(&self) -> &Env {
        self.env
    }
    pub fn info(&self) -> &MessageInfo {
        self.info
    }
    pub fn correlation(&self) -> Correlation {
        self.correlation
    }

    pub fn prepare<O: Operation>(&mut self, request: O::Request) -> RuntimeResult<PreparedWait> {
        if O::KIND.is_empty() {
            return Err(RuntimeError::InvalidKind);
        }
        let encoded = to_json_binary(&request)?;
        let mut wait = O::prepare(self, self.correlation, request)?;
        wait.operation = O::KIND.into();
        wait.request = encoded;
        wait.resolver = self.api.addr_validate(wait.resolver.as_str())?;
        if wait.deadline.reached(self.env) {
            return Err(RuntimeError::DeadlineReached);
        }
        Ok(wait)
    }
}

/// Adapter for a typed durable wait. `decode` must be pure: the runtime calls
/// it during validation and the workflow calls it again during execution.
pub trait Operation {
    const KIND: &'static str;
    type Request: Serialize + DeserializeOwned;
    type Output: Serialize + DeserializeOwned;

    fn prepare(
        ctx: &mut WorkflowCtx<'_>,
        key: Correlation,
        request: Self::Request,
    ) -> RuntimeResult<PreparedWait>;

    fn decode(outcome: Outcome) -> RuntimeResult<Result<Self::Output, WaitError>> {
        match outcome {
            Outcome::Success(payload) => Ok(Ok(from_json(payload)?)),
            Outcome::Error { code, message } => Ok(Err(WaitError::Remote { code, message })),
        }
    }
}

pub fn decode_resolution<O: Operation>(
    resolution: Resolution,
) -> RuntimeResult<Result<O::Output, WaitError>> {
    match resolution {
        Resolution::Outcome(outcome) => O::decode(outcome),
        Resolution::Timeout => Ok(Err(WaitError::Timeout)),
    }
}
