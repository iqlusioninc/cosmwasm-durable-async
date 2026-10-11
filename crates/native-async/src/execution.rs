//! Version-one private sudo ABI. Bytes use CosmWasm's base64 JSON encoding.
use cosmwasm_std::{Binary, Coin, CosmosMsg, CustomMsg, CustomQuery, Decimal};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskProgram {
    pub kind: String,
    pub version: u32,
    pub input: Binary,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PollRequest {
    pub task_id: u64,
    /// Sequence of the operation being resumed; the first new wait is 1.
    pub wait_sequence: u64,
    pub kind: String,
    pub version: u32,
    pub step: PollStep,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PollStep {
    Start {
        input: Binary,
    },
    Resume {
        state: Binary,
        resolution: Resolution,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Success { data: Binary },
    Error { code: String, message: String },
    Timeout {},
    Timer {},
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Wait {
    Height {
        height: u64,
    },
    Operation {
        resolver: String,
        deadline: u64,
        max_result_bytes: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PollOutcome {
    Pending { state: Binary, wait: Wait },
    Yield { state: Binary },
    Complete { output: Binary },
    Fail { error: String },
}

/// Encoded in sudo response.data. Outer sudo messages MUST be empty: the host
/// stages the pending wait before atomically dispatching these messages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PollResponse {
    pub outcome: PollOutcome,
    #[serde(default)]
    pub messages: Vec<CosmosMsg>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskSudoMsg {
    PollTask(PollRequest),
}

/// Executable registration and delivery, separate from the legacy funding-only API.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMsg {
    Register {
        control: String,
        refund: String,
        gas_per_poll: u64,
        max_gas_price: Decimal,
        expires_at: u64,
        funds: Coin,
        program: TaskProgram,
        allocated_bytes: u64,
    },
    /// Owner contract forwards its authenticated MessageInfo.sender as resolver.
    Deliver {
        task_id: u64,
        wait_sequence: u64,
        resolver: String,
        resolution: Resolution,
    },
    Funding(crate::SchedulerMsg),
}
impl CustomMsg for RuntimeMsg {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeQuery {
    Funding(crate::SchedulerQuery),
    Task { contract: String, task_id: u64 },
}
impl CustomQuery for RuntimeQuery {}
