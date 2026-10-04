//! Illustrative delayed service: no real payment or shipment is performed.
use cosmwasm_std::{
    entry_point, to_json_binary, Addr, Binary, Deps, DepsMut, Env, MessageInfo, Order, Response,
    StdError, StdResult, WasmMsg,
};
use cw_storage_plus::{Bound, Item, Map};
use durable_runtime::{Correlation, Instance, Outcome, Status};
use serde::{Deserialize, Serialize};
const OWNER: Item<Addr> = Item::new("owner");
const WORKFLOW: Item<Addr> = Item::new("workflow");
const NEXT: Item<u64> = Item::new("next");
const REQUESTS: Map<u64, Pending> = Map::new("pending");
const CORRELATIONS: Map<(u64, u64), bool> = Map::new("seen");
#[derive(Debug, Serialize, Deserialize)]
pub struct InstantiateMsg {}
#[derive(Debug, Serialize, Deserialize)]
pub enum ExecuteMsg {
    Bind {
        workflow: String,
    },
    Begin {
        correlation: Correlation,
        request: Binary,
    },
    Deliver {
        id: u64,
        outcome: Outcome,
    },
    Prune {
        id: u64,
    },
}
#[derive(Debug, Serialize, Deserialize)]
pub enum QueryMsg {
    Pending {
        start_after: Option<u64>,
        limit: Option<u32>,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub id: u64,
    pub correlation: Correlation,
    pub request: Binary,
}
#[derive(Serialize)]
enum Callback {
    Resume {
        workflow_id: u64,
        wait_sequence: u64,
        outcome: Outcome,
    },
}
#[derive(Serialize)]
enum WorkflowQuery {
    Instance { workflow_id: u64 },
}
fn error(msg: &str) -> StdError {
    StdError::generic_err(msg)
}
#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    _: Env,
    info: MessageInfo,
    _: InstantiateMsg,
) -> StdResult<Response> {
    if !info.funds.is_empty() {
        return Err(error("no funds accepted"));
    }
    OWNER.save(deps.storage, &info.sender)?;
    NEXT.save(deps.storage, &1)?;
    Ok(Response::new())
}
#[entry_point]
pub fn execute(deps: DepsMut, _: Env, info: MessageInfo, msg: ExecuteMsg) -> StdResult<Response> {
    if !info.funds.is_empty() {
        return Err(error("no funds accepted"));
    }
    match msg {
        ExecuteMsg::Bind { workflow } => {
            if OWNER.load(deps.storage)? != info.sender {
                return Err(error("unauthorized"));
            }
            if WORKFLOW.may_load(deps.storage)?.is_some() {
                return Err(error("already bound"));
            }
            WORKFLOW.save(deps.storage, &deps.api.addr_validate(&workflow)?)?;
            Ok(Response::new())
        }
        ExecuteMsg::Begin {
            correlation,
            request,
        } => {
            if WORKFLOW.load(deps.storage)? != info.sender {
                return Err(error("unauthorized workflow"));
            }
            if request.len() > 64 * 1024 {
                return Err(error("request too large"));
            }
            let key = (correlation.workflow_id, correlation.wait_sequence);
            if CORRELATIONS.has(deps.storage, key) {
                return Err(error("duplicate correlation"));
            }
            let id = NEXT.load(deps.storage)?;
            let next = id.checked_add(1).ok_or_else(|| error("id overflow"))?;
            REQUESTS.save(
                deps.storage,
                id,
                &Pending {
                    id,
                    correlation,
                    request,
                },
            )?;
            CORRELATIONS.save(deps.storage, key, &true)?;
            NEXT.save(deps.storage, &next)?;
            Ok(Response::new().add_attribute("request_id", id.to_string()))
        }
        ExecuteMsg::Deliver { id, outcome } => {
            if OWNER.load(deps.storage)? != info.sender {
                return Err(error("unauthorized operator"));
            }
            let p = REQUESTS.load(deps.storage, id)?;
            REQUESTS.remove(deps.storage, id);
            // A failed callback rolls back this removal with the entire transaction.
            Ok(Response::new().add_message(WasmMsg::Execute {
                contract_addr: WORKFLOW.load(deps.storage)?.into_string(),
                msg: to_json_binary(&Callback::Resume {
                    workflow_id: p.correlation.workflow_id,
                    wait_sequence: p.correlation.wait_sequence,
                    outcome,
                })?,
                funds: vec![],
            }))
        }
        ExecuteMsg::Prune { id } => {
            let p = REQUESTS.load(deps.storage, id)?;
            let instance: Instance = deps.querier.query_wasm_smart(
                WORKFLOW.load(deps.storage)?,
                &WorkflowQuery::Instance {
                    workflow_id: p.correlation.workflow_id,
                },
            )?;
            match instance.status {
                Status::Completed { .. } | Status::Failed { .. } => {}
                Status::Waiting { wait, .. } if wait.sequence > p.correlation.wait_sequence => {}
                _ => return Err(error("request is still live")),
            }
            REQUESTS.remove(deps.storage, id);
            Ok(Response::new())
        }
    }
}
#[entry_point]
pub fn query(deps: Deps, _: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Pending { start_after, limit } => {
            let limit = limit.unwrap_or(30).clamp(1, 100) as usize;
            let pending = REQUESTS
                .range(
                    deps.storage,
                    start_after.map(Bound::exclusive),
                    None,
                    Order::Ascending,
                )
                .take(limit)
                .map(|r| r.map(|(_, v)| v))
                .collect::<StdResult<Vec<_>>>()?;
            to_json_binary(&pending)
        }
    }
}
