//! Harness service fixture. Records the requesting contract; only its operator
//! may release a result, and the callback owner cannot be supplied by a caller.
use cosmwasm_std::{
    entry_point, from_json, to_json_binary, Addr, Binary, DepsMut, Env, MessageInfo, Response,
    StdError, StdResult, WasmMsg,
};
use durable_runtime::{Correlation, Outcome};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct InstantiateMsg {}
#[derive(Serialize, Deserialize)]
pub enum ExecuteMsg {
    Begin {
        correlation: Correlation,
        request: Binary,
    },
    Release {
        correlation: Correlation,
        outcome: Outcome,
    },
}
#[derive(Serialize)]
enum Callback {
    Resume {
        workflow_id: u64,
        wait_sequence: u64,
        outcome: Outcome,
    },
}
fn key(owner: &Addr, correlation: Correlation) -> Vec<u8> {
    format!(
        "request/{}/{}/{}",
        owner, correlation.workflow_id, correlation.wait_sequence
    )
    .into_bytes()
}

#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    _: Env,
    info: MessageInfo,
    _: InstantiateMsg,
) -> StdResult<Response> {
    deps.storage
        .set(b"operator", &to_json_binary(&info.sender)?);
    Ok(Response::new())
}

#[entry_point]
pub fn execute(deps: DepsMut, _: Env, info: MessageInfo, msg: ExecuteMsg) -> StdResult<Response> {
    if !info.funds.is_empty() {
        return Err(StdError::generic_err("unexpected funds"));
    }
    match msg {
        ExecuteMsg::Begin {
            correlation,
            request,
        } => {
            let request_key = key(&info.sender, correlation);
            if deps.storage.get(&request_key).is_some() {
                return Err(StdError::generic_err("duplicate request"));
            }
            deps.storage.set(&request_key, &request);
            // Fixture allows one owner for each host ID/sequence, established by Begin.
            deps.storage.set(
                format!(
                    "owner/{}/{}",
                    correlation.workflow_id, correlation.wait_sequence
                )
                .as_bytes(),
                &to_json_binary(&info.sender)?,
            );
            Ok(Response::new())
        }
        ExecuteMsg::Release {
            correlation,
            outcome,
        } => {
            let operator: Addr = from_json(deps.storage.get(b"operator").unwrap())?;
            if operator != info.sender {
                return Err(StdError::generic_err("unauthorized operator"));
            }
            let owner_key = format!(
                "owner/{}/{}",
                correlation.workflow_id, correlation.wait_sequence
            );
            let owner: Addr = from_json(
                deps.storage
                    .get(owner_key.as_bytes())
                    .ok_or_else(|| StdError::not_found("request"))?,
            )?;
            deps.storage.remove(owner_key.as_bytes());
            Ok(Response::new().add_message(WasmMsg::Execute {
                contract_addr: owner.into_string(),
                funds: vec![],
                msg: to_json_binary(&Callback::Resume {
                    workflow_id: correlation.workflow_id,
                    wait_sequence: correlation.wait_sequence,
                    outcome,
                })?,
            }))
        }
    }
}
