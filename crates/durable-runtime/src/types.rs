use cosmwasm_std::{Addr, Binary, CosmosMsg, Env, StdError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type RuntimeResult<T> = Result<T, RuntimeError>;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("{0}")]
    Std(#[from] StdError),
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("workflow instance {0} does not exist")]
    MissingInstance(u64),
    #[error("workflow instance is not waiting")]
    NotWaiting,
    #[error("unauthorized resolver")]
    Unauthorized,
    #[error("stale wait sequence: expected {expected}, received {received}")]
    StaleWait { expected: u64, received: u64 },
    #[error("deadline has been reached")]
    DeadlineReached,
    #[error("deadline has not been reached")]
    DeadlineNotReached,
    #[error("unsupported workflow {kind} version {version}")]
    UnsupportedWorkflow { kind: String, version: u32 },
    #[error("duplicate registration for {kind} version {version}")]
    DuplicateRegistration { kind: String, version: u32 },
    #[error("workflow version must be positive")]
    InvalidVersion,
    #[error("workflow or operation kind must not be empty")]
    InvalidKind,
    #[error("{limit} limit exceeded: maximum {max}, actual {actual}")]
    LimitExceeded {
        limit: &'static str,
        max: usize,
        actual: usize,
    },
    #[error("active workflow limit reached: maximum {0}")]
    ActiveLimitReached(u64),
    #[error("runtime counter overflow")]
    CounterOverflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Correlation {
    pub workflow_id: u64,
    pub wait_sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Deadline {
    Height(u64),
    /// Unix time in whole seconds.
    Time(u64),
}

impl Deadline {
    pub fn reached(&self, env: &Env) -> bool {
        match self {
            Self::Height(height) => env.block.height >= *height,
            Self::Time(seconds) => env.block.time.seconds() >= *seconds,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Success(Binary),
    Error { code: String, message: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    Outcome(Outcome),
    Timeout,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum WaitError {
    #[error("remote operation error {code}: {message}")]
    Remote { code: String, message: String },
    #[error("operation timed out")]
    Timeout,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PreparedWait {
    pub operation: String,
    pub request: Binary,
    pub resolver: Addr,
    pub deadline: Deadline,
    pub messages: Vec<CosmosMsg>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Transition {
    Wait { state: Binary, wait: PreparedWait },
    Completed(Binary),
    Failed(Binary),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitRecord {
    pub sequence: u64,
    pub operation: String,
    pub request: Binary,
    pub resolver: Addr,
    pub deadline: Deadline,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Waiting {
        state: Binary,
        wait: WaitRecord,
    },
    Completed {
        output: Binary,
    },
    Failed {
        error: Binary,
    },
    /// Uncommitted consumption marker. Contract callers must propagate runtime
    /// errors so the CosmWasm host rolls back this marker and segment writes.
    Transitioning,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub workflow_id: u64,
    pub kind: String,
    pub version: u32,
    pub creator: Addr,
    pub start_height: u64,
    pub status: Status,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub max_active: u64,
    pub max_checkpoint_bytes: usize,
    pub max_request_bytes: usize,
    pub max_callback_bytes: usize,
    pub max_result_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_active: 1_000,
            max_checkpoint_bytes: 64 * 1_024,
            max_request_bytes: 64 * 1_024,
            max_callback_bytes: 64 * 1_024,
            max_result_bytes: 64 * 1_024,
        }
    }
}
