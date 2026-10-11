//! Real-VM acceptance fixture; never deploy as an application contract.
use cosmwasm_std::{
    entry_point, to_json_binary, Binary, CosmosMsg, Decimal, Deps, DepsMut, Env, MessageInfo,
    Response, StdError, StdResult, WasmMsg,
};
use native_async::{
    PollOutcome, PollResponse, PollStep, Resolution, RuntimeMsg, SchedulerMsg, TaskProgram,
    TaskSudoMsg, Wait,
};
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
pub struct Empty {}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Execute {
    Start { gas_per_poll: u64, expires_at: u64 },
    Deliver { task_id: u64, wait_sequence: u64 },
    Mode { value: String },
    Resume { task_id: u64 },
    Cancel { task_id: u64 },
    Burn {},
}
#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    _: Env,
    info: MessageInfo,
    _: Empty,
) -> StdResult<Response<RuntimeMsg>> {
    deps.storage.set(b"operator", info.sender.as_bytes());
    deps.storage.set(b"writes", &0u64.to_be_bytes());
    Ok(Response::new())
}
#[entry_point]
pub fn execute(
    deps: DepsMut,
    _: Env,
    info: MessageInfo,
    msg: Execute,
) -> StdResult<Response<RuntimeMsg>> {
    if deps.storage.get(b"operator").as_deref() != Some(info.sender.as_bytes()) {
        return Err(StdError::generic_err("operator required"));
    }
    let custom = match msg {
        Execute::Burn {} => loop {
            std::hint::black_box(deps.storage.get(b"writes"));
        },
        Execute::Start {
            gas_per_poll,
            expires_at,
        } => RuntimeMsg::Register {
            control: info.sender.to_string(),
            refund: info.sender.to_string(),
            gas_per_poll,
            max_gas_price: Decimal::one(),
            expires_at,
            funds: info
                .funds
                .first()
                .cloned()
                .ok_or_else(|| StdError::generic_err("funding required"))?,
            program: TaskProgram {
                kind: "fault".into(),
                version: 1,
                input: Binary::default(),
            },
            allocated_bytes: 8192,
        },
        Execute::Deliver {
            task_id,
            wait_sequence,
        } => RuntimeMsg::Deliver {
            task_id,
            wait_sequence,
            resolver: info.sender.to_string(),
            resolution: Resolution::Success {
                data: Binary::from(b"result"),
            },
        },
        Execute::Resume { task_id } => RuntimeMsg::Funding(SchedulerMsg::ResumeTask {
            task_id,
            actor: info.sender.to_string(),
        }),
        Execute::Cancel { task_id } => RuntimeMsg::Funding(SchedulerMsg::CancelTask {
            task_id,
            actor: info.sender.to_string(),
        }),
        Execute::Mode { value } => {
            deps.storage.set(b"mode", value.as_bytes());
            return Ok(Response::new());
        }
    };
    Ok(Response::new().add_message(CosmosMsg::Custom(custom)))
}
#[entry_point]
pub fn sudo(deps: DepsMut, env: Env, msg: TaskSudoMsg) -> StdResult<Response> {
    let TaskSudoMsg::PollTask(request) = msg;
    let writes = u64::from_be_bytes(deps.storage.get(b"writes").unwrap().try_into().unwrap());
    deps.storage.set(b"writes", &(writes + 1).to_be_bytes());
    let outcome = match request.step {
        PollStep::Start { .. } => PollOutcome::Pending {
            state: Binary::from(b"committed continuation"),
            wait: Wait::Operation {
                resolver: String::from_utf8(deps.storage.get(b"operator").unwrap()).unwrap(),
                deadline: env.block.height + 50,
                max_result_bytes: 256,
            },
        },
        PollStep::Resume { .. } => {
            let mode = String::from_utf8(deps.storage.get(b"mode").unwrap_or_default()).unwrap();
            match mode.as_str() {
                "trap" => panic!("fault after write"),
                "out_of_gas" => loop {
                    std::hint::black_box(deps.storage.get(b"writes"));
                },
                "invalid" => return Ok(Response::new().set_data(Binary::from(b"invalid ABI"))),
                "outbound" | "fail_outbound" => {
                    return Ok(Response::new().set_data(to_json_binary(&PollResponse {
                        outcome: if mode == "fail_outbound" {
                            PollOutcome::Fail {
                                error: "intentional failure".into(),
                            }
                        } else {
                            PollOutcome::Complete {
                                output: Binary::default(),
                            }
                        },
                        messages: vec![WasmMsg::Execute {
                            contract_addr: env.contract.address.into_string(),
                            msg: Binary::from(b"invalid execute JSON"),
                            funds: vec![],
                        }
                        .into()],
                    })?))
                }
                _ => PollOutcome::Complete {
                    output: Binary::from(b"done"),
                },
            }
        }
    };
    Ok(Response::new().set_data(to_json_binary(&PollResponse {
        outcome,
        messages: vec![],
    })?))
}
#[entry_point]
pub fn query(deps: Deps, _: Env, _: Empty) -> StdResult<Binary> {
    let writes = u64::from_be_bytes(deps.storage.get(b"writes").unwrap().try_into().unwrap());
    #[derive(Serialize)]
    struct Count {
        writes: u64,
    }
    to_json_binary(&Count { writes })
}
#[entry_point]
pub fn migrate(_: DepsMut, _: Env, _: Empty) -> StdResult<Response> {
    Ok(Response::new())
}
