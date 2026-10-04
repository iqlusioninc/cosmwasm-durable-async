//! Example durable workflow with execute-based payment and shipment callbacks.

use cosmwasm_std::{
    entry_point, from_json, to_json_binary, Addr, Binary, Deps, DepsMut, Env, MessageInfo,
    Response, StdError, StdResult, WasmMsg,
};
use durable_macro::durable_workflow;
use durable_runtime::{
    active_count, expire, load_instance, registration, resume, start, Correlation, Deadline,
    Limits, Operation, Outcome, PreparedWait, Registry, RuntimeError, RuntimeResult, WaitError,
    WorkflowCtx,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const CONFIG: &[u8] = b"example/config";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstantiateMsg {
    pub payment_service: String,
    pub shipment_service: String,
    pub deadline_blocks: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecuteMsg {
    Start {
        order: Order,
    },
    Resume {
        workflow_id: u64,
        wait_sequence: u64,
        outcome: Outcome,
    },
    Expire {
        workflow_id: u64,
        wait_sequence: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryMsg {
    Instance { workflow_id: u64 },
    PaymentSeen { order_id: u64 },
    ActiveCount {},
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
    if msg.deadline_blocks == 0 {
        return Err(StdError::generic_err("deadline_blocks must be positive").into());
    }
    let cfg = Config {
        owner: info.sender,
        payment_service: deps.api.addr_validate(&msg.payment_service)?,
        shipment_service: deps.api.addr_validate(&msg.shipment_service)?,
        deadline_blocks: msg.deadline_blocks,
    };
    deps.storage.set(CONFIG, &to_json_binary(&cfg)?);
    Ok(Response::new())
}

#[entry_point]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    if !info.funds.is_empty() {
        return Err(ContractError::UnexpectedFunds);
    }
    let limits = Limits::default();
    match msg {
        ExecuteMsg::Start { order } => {
            if config(deps.storage)?.owner != info.sender {
                return Err(ContractError::Unauthorized);
            }
            let (id, response) =
                start::<fulfill::Workflow>(deps, env, info, fulfill::Input { order }, &limits)?;
            Ok(response.set_data(to_json_binary(&id)?))
        }
        ExecuteMsg::Resume {
            workflow_id,
            wait_sequence,
            outcome,
        } => Ok(resume(
            deps,
            env,
            info,
            workflow_id,
            wait_sequence,
            outcome,
            &registry()?,
            &limits,
        )?),
        ExecuteMsg::Expire {
            workflow_id,
            wait_sequence,
        } => Ok(expire(
            deps,
            env,
            info,
            workflow_id,
            wait_sequence,
            &registry()?,
            &limits,
        )?),
    }
}

#[entry_point]
pub fn query(deps: Deps, _: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Instance { workflow_id } => to_json_binary(
            &load_instance(deps.storage, workflow_id)
                .map_err(|e| StdError::generic_err(e.to_string()))?,
        ),
        QueryMsg::PaymentSeen { order_id } => {
            let payment: Option<Payment> = deps
                .storage
                .get(&payment_key(order_id))
                .map(from_json)
                .transpose()?;
            to_json_binary(&payment)
        }
        QueryMsg::ActiveCount {} => to_json_binary(
            &active_count(deps.storage, "fulfill", 1)
                .map_err(|e| StdError::generic_err(e.to_string()))?,
        ),
    }
}
