use cosmwasm_std::{Coin, CustomMsg, CustomQuery, Decimal, Uint128};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub type TaskId = u64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AsyncExecuteMsg {
    FundTask { task_id: TaskId },
    ResumeTask { task_id: TaskId },
    CancelTask { task_id: TaskId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AsyncQueryMsg {
    FundingConfig {},
    TaskFunding { task_id: TaskId },
}

/// Contract-to-host messages. The host derives ownership from the executing
/// contract, never from the payload. Actor/contributor fields are assertions by
/// that contract; contracts must forward MessageInfo.sender faithfully.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerMsg {
    RegisterTask {
        control: String,
        refund: String,
        gas_per_poll: u64,
        max_gas_price: Decimal,
        expires_at: u64,
        funds: Coin,
    },
    FundTask {
        task_id: TaskId,
        contributor: String,
        funds: Coin,
    },
    ResumeTask {
        task_id: TaskId,
        actor: String,
    },
    CancelTask {
        task_id: TaskId,
        actor: String,
    },
}
impl CustomMsg for SchedulerMsg {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerQuery {
    FundingConfig {},
    TaskFunding { contract: String, task_id: TaskId },
}
impl CustomQuery for SchedulerQuery {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FundingConfig {
    pub denom: String,
    pub gas_price: Decimal,
    pub max_gas_per_poll: u64,
    pub max_lifetime_blocks: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResumeState {
    Runnable,
    Waiting,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    InsufficientFunds,
    PriceLimit,
    OutOfGas,
    Trap,
    MessageFailure,
    InvalidResponse,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Runnable,
    Waiting,
    Paused {
        reason: PauseReason,
        resume: ResumeState,
    },
    Cancelled,
    Expired,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskFunding {
    pub task_id: TaskId,
    pub contract: String,
    pub control: String,
    pub refund: String,
    pub available: Uint128,
    #[serde(default)]
    pub reserved: Uint128,
    #[serde(default)]
    pub cumulative_execution_fees: Uint128,
    #[serde(default)]
    pub allocated_bytes: u64,
    #[serde(default)]
    pub storage_lease_paid: Uint128,
    pub gas_per_poll: u64,
    pub max_gas_price: Decimal,
    pub max_attempt_charge: Uint128,
    pub expires_at: u64,
    pub status: TaskStatus,
}
