//! Example durable workflow with execute-based payment and shipment callbacks.

use cosmwasm_std::{
    entry_point, from_json, to_json_binary, Addr, Binary, Deps, DepsMut, Env, MessageInfo,
    Response, StdError, StdResult, WasmMsg,
};
use durable_macro::durable_workflow;
use durable_runtime::{
    registration, Correlation, Deadline, Operation, PreparedWait, Registry, RuntimeError,
    RuntimeResult, WaitError, WorkflowCtx,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const CONFIG: &[u8] = b"example/config";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstantiateMsg {
    pub payment_service: String,
    pub shipment_service: String,
    pub deadline_blocks: u64,
    pub fee_denom: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecuteMsg {
    Start {
        order: Order,
        business: cosmwasm_std::Coin,
        gas: cosmwasm_std::Coin,
        gas_per_poll: u64,
        max_gas_price: cosmwasm_std::Decimal,
        expires_at: u64,
        allocated_bytes: u64,
    },
    Resume {
        workflow_id: u64,
        wait_sequence: u64,
        outcome: durable_runtime::Outcome,
    },
    Async(native_async::AsyncExecuteMsg),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryMsg {
    Task { task_id: u64 },
    PaymentSeen { order_id: u64 },
    Async(native_async::AsyncQueryMsg),
}

/// A service stores the correlation key and sends a Resume in a later transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceMsg {
    Begin {
        correlation: Correlation,
        request: Binary,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub id: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Payment {
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shipment {
    pub tracking: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Receipt {
    pub order_id: u64,
    pub shipment: Shipment,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Error)]
pub enum WorkflowError {
    #[error("operation failed: {0}")]
    Wait(#[from] WaitError),
    #[error("payment reference is empty")]
    PaymentRejected,
    #[error("storage error: {0}")]
    Storage(String),
}

impl From<StdError> for WorkflowError {
    fn from(value: StdError) -> Self {
        Self::Storage(value.to_string())
    }
}

#[derive(Debug, Error)]
pub enum ContractError {
    #[error("{0}")]
    Runtime(#[from] RuntimeError),
    #[error("{0}")]
    Std(#[from] StdError),
    #[error("only the owner can start a workflow")]
    Unauthorized,
    #[error("this contract does not accept funds")]
    UnexpectedFunds,
}

#[derive(Serialize, Deserialize)]
struct Config {
    owner: Addr,
    payment_service: Addr,
    shipment_service: Addr,
    deadline_blocks: u64,
    fee_denom: String,
}

#[derive(Serialize, Deserialize)]
pub struct PaymentRequest {
    pub order_id: u64,
}

#[derive(Serialize, Deserialize)]
pub struct ShipmentRequest {
    pub order_id: u64,
    pub payment: Payment,
}

pub struct PaymentReceived;
pub struct ShipmentReceived;

fn config(storage: &dyn cosmwasm_std::Storage) -> StdResult<Config> {
    from_json(
        storage
            .get(CONFIG)
            .ok_or_else(|| StdError::not_found("config"))?,
    )
}

fn prepare_request<T: Serialize>(
    ctx: &mut WorkflowCtx<'_>,
    key: Correlation,
    request: &T,
    payment: bool,
) -> RuntimeResult<PreparedWait> {
    let cfg = config(ctx.storage())?;
    let resolver = if payment {
        cfg.payment_service
    } else {
        cfg.shipment_service
    };
    let height = ctx
        .env()
        .block
        .height
        .checked_add(cfg.deadline_blocks)
        .ok_or_else(|| RuntimeError::Validation("deadline overflow".into()))?;
    let request = to_json_binary(request)?;
    let message = WasmMsg::Execute {
        contract_addr: resolver.to_string(),
        msg: to_json_binary(&ServiceMsg::Begin {
            correlation: key,
            request: request.clone(),
        })?,
        funds: vec![],
    };
    Ok(PreparedWait {
        operation: String::new(), // WorkflowCtx::prepare sets the adapter kind.
        request,
        resolver,
        deadline: Deadline::Height(height),
        messages: vec![message.into()],
    })
}

impl Operation for PaymentReceived {
    const KIND: &'static str = "payment_received";
    type Request = PaymentRequest;
    type Output = Payment;

    fn prepare(
        ctx: &mut WorkflowCtx<'_>,
        key: Correlation,
        request: PaymentRequest,
    ) -> RuntimeResult<PreparedWait> {
        prepare_request(ctx, key, &request, true)
    }
}

impl Operation for ShipmentReceived {
    const KIND: &'static str = "shipment_received";
    type Request = ShipmentRequest;
    type Output = Shipment;

    fn prepare(
        ctx: &mut WorkflowCtx<'_>,
        key: Correlation,
        request: ShipmentRequest,
    ) -> RuntimeResult<PreparedWait> {
        prepare_request(ctx, key, &request, false)
    }
}

fn payment_key(order_id: u64) -> Vec<u8> {
    format!("example/payment/{order_id}").into_bytes()
}

fn record_payment(
    ctx: &mut WorkflowCtx<'_>,
    order_id: u64,
    payment: &Payment,
) -> Result<(), WorkflowError> {
    ctx.storage()
        .set(&payment_key(order_id), &to_json_binary(payment)?);
    if payment.reference.is_empty() {
        return Err(WorkflowError::PaymentRejected);
    }
    Ok(())
}

#[durable_workflow(kind = "fulfill", version = 1)]
pub async fn fulfill(ctx: WorkflowCtx, order: Order) -> Result<Receipt, WorkflowError> {
    let payment: Payment = ctx
        .wait::<PaymentReceived>(PaymentRequest { order_id: order.id })
        .checkpoint(order)
        .await?;

    record_payment(ctx, order.id, &payment)?;

    let shipment: Shipment = ctx
        .wait::<ShipmentReceived>(ShipmentRequest {
            order_id: order.id,
            payment,
        })
        .checkpoint(order)
        .await?;

    Ok(Receipt {
        order_id: order.id,
        shipment,
    })
}

fn registry() -> RuntimeResult<Registry> {
    Registry::new(vec![registration::<fulfill::Workflow>()])
}

use cosmwasm_std::{CosmosMsg, QueryRequest, Reply, SubMsg};
use native_async::{
    RuntimeMsg, RuntimeQuery, SchedulerMsg, SchedulerQuery, TaskProgram, TaskSudoMsg,
};
const REGISTER_REPLY: u64 = 1;

#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    _: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    if !info.funds.is_empty() {
        return Err(ContractError::UnexpectedFunds);
    }
    if msg.deadline_blocks == 0 || msg.fee_denom.is_empty() {
        return Err(StdError::generic_err("invalid config").into());
    }
    deps.storage.set(
        CONFIG,
        &to_json_binary(&Config {
            owner: info.sender,
            payment_service: deps.api.addr_validate(&msg.payment_service)?,
            shipment_service: deps.api.addr_validate(&msg.shipment_service)?,
            deadline_blocks: msg.deadline_blocks,
            fee_denom: msg.fee_denom,
        })?,
    );
    Ok(Response::new())
}

#[entry_point]
pub fn execute(
    deps: DepsMut<RuntimeQuery>,
    _: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response<RuntimeMsg>, ContractError> {
    let cfg = config(deps.storage)?;
    match msg {
        ExecuteMsg::Start {
            order,
            business,
            gas,
            gas_per_poll,
            max_gas_price,
            expires_at,
            allocated_bytes,
        } => {
            if cfg.owner != info.sender {
                return Err(ContractError::Unauthorized);
            }
            if gas.denom != cfg.fee_denom || gas.amount.is_zero() {
                return Err(StdError::generic_err("invalid gas deposit").into());
            }
            let mut expected = std::collections::BTreeMap::new();
            for coin in [&business, &gas] {
                if !coin.amount.is_zero() {
                    let amount = expected
                        .entry(coin.denom.clone())
                        .or_insert(cosmwasm_std::Uint128::zero());
                    *amount = amount
                        .checked_add(coin.amount)
                        .map_err(StdError::overflow)?;
                }
            }
            let mut attached = std::collections::BTreeMap::new();
            for coin in &info.funds {
                if coin.amount.is_zero()
                    || attached.insert(coin.denom.clone(), coin.amount).is_some()
                {
                    return Err(StdError::generic_err("invalid attached funds").into());
                }
            }
            if expected != attached {
                return Err(StdError::generic_err(
                    "business and gas split does not match attached funds",
                )
                .into());
            }
            // The business allocation remains in contract custody; only gas enters host escrow.
            Ok(Response::new().add_submessage(SubMsg::reply_on_success(
                CosmosMsg::Custom(RuntimeMsg::Register {
                    control: info.sender.to_string(),
                    refund: info.sender.to_string(),
                    gas_per_poll,
                    max_gas_price,
                    expires_at,
                    funds: gas,
                    allocated_bytes,
                    program: TaskProgram {
                        kind: "fulfill".into(),
                        version: 1,
                        input: to_json_binary(&fulfill::Input { order })?,
                    },
                }),
                REGISTER_REPLY,
            )))
        }
        ExecuteMsg::Resume {
            workflow_id,
            wait_sequence,
            outcome,
        } => {
            if !info.funds.is_empty() {
                return Err(ContractError::UnexpectedFunds);
            }
            let resolution = match outcome {
                durable_runtime::Outcome::Success(data) => {
                    native_async::Resolution::Success { data }
                }
                durable_runtime::Outcome::Error { code, message } => {
                    native_async::Resolution::Error { code, message }
                }
            };
            Ok(
                Response::new().add_message(CosmosMsg::Custom(RuntimeMsg::Deliver {
                    task_id: workflow_id,
                    wait_sequence,
                    resolver: info.sender.to_string(),
                    resolution,
                })),
            )
        }
        ExecuteMsg::Async(msg) => {
            let scheduler = match msg {
                native_async::AsyncExecuteMsg::FundTask { task_id } => SchedulerMsg::FundTask {
                    task_id,
                    contributor: info.sender.to_string(),
                    funds: native_async::attached_gas(&info, &cfg.fee_denom)?,
                },
                native_async::AsyncExecuteMsg::ResumeTask { task_id } => {
                    if !info.funds.is_empty() {
                        return Err(ContractError::UnexpectedFunds);
                    }
                    SchedulerMsg::ResumeTask {
                        task_id,
                        actor: info.sender.to_string(),
                    }
                }
                native_async::AsyncExecuteMsg::CancelTask { task_id } => {
                    if !info.funds.is_empty() {
                        return Err(ContractError::UnexpectedFunds);
                    }
                    SchedulerMsg::CancelTask {
                        task_id,
                        actor: info.sender.to_string(),
                    }
                }
            };
            Ok(Response::new().add_message(CosmosMsg::Custom(RuntimeMsg::Funding(scheduler))))
        }
    }
}

#[entry_point]
pub fn sudo(deps: DepsMut, env: Env, msg: TaskSudoMsg) -> Result<Response, ContractError> {
    let TaskSudoMsg::PollTask(request) = msg;
    Ok(durable_runtime::native::poll(
        deps,
        env,
        request,
        &registry()?,
        65536,
    )?)
}

/// Register custom-message results are raw host JSON, unlike protobuf-wrapped
/// Wasm execute replies. Propagate the host's task ID, never the reply ID.
#[entry_point]
pub fn reply(_: DepsMut<RuntimeQuery>, _: Env, reply: Reply) -> StdResult<Response<RuntimeMsg>> {
    if reply.id != REGISTER_REPLY {
        return Err(StdError::generic_err("unknown reply id"));
    }
    let result = reply.result.into_result().map_err(StdError::generic_err)?;
    #[allow(deprecated)]
    let data = result
        .data
        .ok_or_else(|| StdError::generic_err("registration reply missing task ID"))?;
    let task_id: u64 = from_json(&data)?;
    Ok(Response::new().set_data(to_json_binary(&task_id)?))
}

#[entry_point]
pub fn query(deps: Deps<RuntimeQuery>, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Task { task_id } => {
            let request = to_json_binary(&QueryRequest::Custom(RuntimeQuery::Task {
                contract: env.contract.address.to_string(),
                task_id,
            }))?;
            match deps.querier.raw_query(&request) {
                cosmwasm_std::SystemResult::Ok(cosmwasm_std::ContractResult::Ok(data)) => Ok(data),
                cosmwasm_std::SystemResult::Ok(cosmwasm_std::ContractResult::Err(err)) => {
                    Err(StdError::generic_err(err))
                }
                cosmwasm_std::SystemResult::Err(err) => Err(StdError::generic_err(err.to_string())),
            }
        }
        QueryMsg::PaymentSeen { order_id } => {
            let payment: Option<Payment> = deps
                .storage
                .get(&payment_key(order_id))
                .map(from_json)
                .transpose()?;
            to_json_binary(&payment)
        }
        QueryMsg::Async(msg) => match msg {
            native_async::AsyncQueryMsg::FundingConfig {} => {
                to_json_binary(&deps.querier.query::<native_async::FundingConfig>(
                    &QueryRequest::Custom(RuntimeQuery::Funding(SchedulerQuery::FundingConfig {})),
                )?)
            }
            native_async::AsyncQueryMsg::TaskFunding { task_id } => {
                to_json_binary(&deps.querier.query::<native_async::TaskFunding>(
                    &QueryRequest::Custom(RuntimeQuery::Funding(SchedulerQuery::TaskFunding {
                        contract: env.contract.address.to_string(),
                        task_id,
                    })),
                )?)
            }
        },
    }
}
#[cfg(test)]
mod tests;
