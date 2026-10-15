use cosmwasm_std::*;
use cw_multi_test::{Contract, ContractWrapper};
use native_async::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Execute {
    Register { fail: bool },
    Dispatch { task_id: u64, wait_sequence: u64 },
    Callback { task_id: u64, wait_sequence: u64 },
}

pub fn contract() -> Box<dyn Contract<RuntimeMsg, RuntimeQuery>> {
    fn instantiate(
        _: DepsMut<RuntimeQuery>,
        _: Env,
        _: MessageInfo,
        _: Empty,
    ) -> StdResult<Response<RuntimeMsg>> {
        Ok(Response::new())
    }
    fn execute(
        deps: DepsMut<RuntimeQuery>,
        env: Env,
        info: MessageInfo,
        msg: Execute,
    ) -> StdResult<Response<RuntimeMsg>> {
        match msg {
            Execute::Register { fail } => {
                deps.storage.set(b"fail", &to_json_vec(&fail)?);
                Ok(
                    Response::new().add_message(CosmosMsg::Custom(RuntimeMsg::Register {
                        control: info.sender.to_string(),
                        refund: info.sender.to_string(),
                        gas_per_poll: 100,
                        max_gas_price: Decimal::one(),
                        expires_at: env.block.height + 100,
                        funds: coin(1_000, "ugas"),
                        program: TaskProgram {
                            kind: "adapter-fixture".into(),
                            version: 1,
                            input: Binary::default(),
                        },
                        allocated_bytes: 20_000,
                    })),
                )
            }
            Execute::Dispatch {
                task_id,
                wait_sequence,
            } => {
                if info.sender != env.contract.address {
                    return Err(StdError::generic_err("self only"));
                }
                deps.storage
                    .set(b"dispatch-write", b"must be transactional");
                Ok(Response::new()
                    .add_message(BankMsg::Send {
                        to_address: testing::MockApi::default()
                            .addr_make("beneficiary")
                            .to_string(),
                        amount: coins(10, "uinput"),
                    })
                    .add_message(WasmMsg::Execute {
                        contract_addr: env.contract.address.to_string(),
                        msg: to_json_binary(&Execute::Callback {
                            task_id,
                            wait_sequence,
                        })?,
                        funds: vec![],
                    }))
            }
            Execute::Callback {
                task_id,
                wait_sequence,
            } => {
                if info.sender != env.contract.address {
                    return Err(StdError::generic_err("self only"));
                }
                deps.storage
                    .set(b"callback-write", b"must be transactional");
                if from_json::<bool>(deps.storage.get(b"fail").unwrap())? {
                    return Err(StdError::generic_err("fixture rejects callback"));
                }
                Ok(
                    Response::new().add_message(CosmosMsg::Custom(RuntimeMsg::Deliver {
                        task_id,
                        wait_sequence,
                        resolver: info.sender.to_string(),
                        resolution: Resolution::Success {
                            data: Binary::from(b"receipt".as_slice()),
                        },
                    })),
                )
            }
        }
    }
    fn query(deps: Deps<RuntimeQuery>, _: Env, _: Empty) -> StdResult<Binary> {
        to_json_binary(&(
            deps.storage.get(b"dispatch-write"),
            deps.storage.get(b"callback-write"),
            deps.storage.get(b"poll-write"),
        ))
    }
    fn sudo(
        deps: DepsMut<RuntimeQuery>,
        env: Env,
        msg: TaskSudoMsg,
    ) -> StdResult<Response<RuntimeMsg>> {
        let TaskSudoMsg::PollTask(request) = msg;
        deps.storage.set(b"poll-write", b"must be transactional");
        let response = match request.step {
            PollStep::Start { .. } => PollResponse {
                outcome: PollOutcome::Pending {
                    state: Binary::from(b"checkpoint".as_slice()),
                    wait: Wait::Operation {
                        resolver: env.contract.address.to_string(),
                        deadline: env.block.height + 1,
                        max_result_bytes: 2048,
                    },
                },
                messages: vec![WasmMsg::Execute {
                    contract_addr: env.contract.address.to_string(),
                    msg: to_json_binary(&Execute::Dispatch {
                        task_id: request.task_id,
                        wait_sequence: request.wait_sequence + 1,
                    })?,
                    funds: vec![],
                }
                .into()],
            },
            PollStep::Resume { resolution, .. } => {
                if !matches!(resolution, Resolution::Success { .. }) {
                    return Err(StdError::generic_err("expected callback receipt"));
                }
                PollResponse {
                    outcome: PollOutcome::Complete {
                        output: Binary::from(b"done".as_slice()),
                    },
                    messages: vec![],
                }
            }
        };
        Ok(Response::new().set_data(to_json_binary(&response)?))
    }
    Box::new(ContractWrapper::new(execute, instantiate, query).with_sudo(sudo))
}
