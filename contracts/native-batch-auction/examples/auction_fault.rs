//! Test-only wrapper around the real auction. Never deploy in the tutorial.
use cosmwasm_std::*;
use native_async::{RuntimeMsg, RuntimeQuery, TaskSudoMsg};
use native_batch_auction::{contract, error::ContractError, msg, state::*};
use serde::{Deserialize, Serialize};

const FAULT_KEY: &[u8] = b"test-only/auction-fault";

#[derive(Serialize, Deserialize)]
pub struct Fault {
    pub mode: String,
    pub phase: SettlementPhase,
    pub cursor: u16,
}
#[derive(Deserialize)]
#[serde(untagged)]
pub enum ExecuteMsg {
    Fault { fault: Fault },
    Auction(msg::ExecuteMsg),
}

#[entry_point]
pub fn instantiate(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    info: MessageInfo,
    msg: msg::InstantiateMsg,
) -> Result<Response<RuntimeMsg>, ContractError> {
    contract::instantiate(deps, env, info, msg)
}
#[entry_point]
pub fn execute(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response<RuntimeMsg>, ContractError> {
    match msg {
        ExecuteMsg::Auction(msg) => contract::execute(deps, env, info, msg),
        ExecuteMsg::Fault { fault } => {
            if !info.funds.is_empty()
                || info.sender != load_config(deps.storage)?.seller
                || !matches!(fault.mode.as_str(), "off" | "error" | "trap" | "oog")
            {
                return Err(StdError::generic_err("invalid test-only fault control").into());
            }
            deps.storage.set(FAULT_KEY, &to_json_vec(&fault)?);
            Ok(Response::new())
        }
    }
}
#[entry_point]
pub fn query(deps: Deps<RuntimeQuery>, env: Env, msg: msg::QueryMsg) -> StdResult<Binary> {
    contract::query(deps, env, msg)
}
#[entry_point]
pub fn reply(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    msg: Reply,
) -> Result<Response<RuntimeMsg>, ContractError> {
    contract::reply(deps, env, msg)
}
#[entry_point]
pub fn sudo(mut deps: DepsMut, env: Env, msg: TaskSudoMsg) -> Result<Response, ContractError> {
    let fault: Option<Fault> = deps.storage.get(FAULT_KEY).map(from_json).transpose()?;
    let before = load_auction(deps.storage)?;
    let armed = fault.as_ref().is_some_and(|f| {
        f.mode != "off"
            && f.phase == before.settlement.phase
            && f.cursor == before.settlement.cursor
            && env.block.height >= before.terms.reveal_end
    });
    let response = contract::sudo(deps.branch(), env, msg)?;
    if armed {
        // Require actual changed auction and nonempty bid writes, rather than
        // throwing before the real settlement step has run.
        let after = load_auction(deps.storage)?;
        let bid = load_bid(deps.storage, before.settlement.cursor)?;
        if before == after || bid.allocated_lots.is_zero() {
            return Err(
                StdError::generic_err("fault fixture did not reach nonempty real batch").into(),
            );
        }
        // Observable through the VM store observer even when the cache rolls back.
        deps.storage.set(b"test-only/real-batch-reached", b"yes");
        match fault.unwrap().mode.as_str() {
            "error" => {
                return Err(StdError::generic_err("intentional error after real batch").into())
            }
            "trap" => panic!("intentional trap after real batch"),
            "oog" => {
                let mut n = 0u64;
                loop {
                    // Real VM gas exhaustion after the entire real bounded step.
                    deps.storage.set(b"test-only/burn", &n.to_be_bytes());
                    n = n.wrapping_add(1);
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(response)
}
