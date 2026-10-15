use cosmwasm_std::*;
use cw_multi_test::{Contract, ContractWrapper};
use native_async::*;
use serde::{Deserialize, Serialize};

pub const FAULT_KEY: &[u8] = b"dca-test/fault";
pub const PROXY_KEY: &[u8] = b"dca-test/proxy";
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Fault {
    None,
    Omit,
    Duplicate,
    Stale,
    WrongOrder,
    WrongSlice,
    WrongTask,
    WrongSender,
    InputAmount,
    InputDenom,
    OutputDenom,
    Beneficiary,
    BelowMinimum,
    BankFailure,
    DeliveryFailure,
    ReplyFailure,
    ReplyCrossOrder,
    ReplyReplay,
    Pad2048,
    Pad2049,
    RemoteError,
}
fn fault(storage: &dyn Storage) -> StdResult<Fault> {
    storage
        .get(FAULT_KEY)
        .map(from_json)
        .transpose()
        .map(|value| value.unwrap_or(Fault::None))
}

/// Delegates real exchange validation, accounting and receipt storage, then
/// injects a fault into the actual nested message chain.
pub fn faulty_exchange() -> Box<dyn Contract<RuntimeMsg, RuntimeQuery>> {
    fn execute(
        mut deps: DepsMut,
        env: Env,
        info: MessageInfo,
        msg: dca_example_protocol::ExchangeExecuteMsg,
    ) -> StdResult<Response> {
        let mode = fault(deps.storage)?;
        let proxy: Option<String> = deps.storage.get(PROXY_KEY).map(from_json).transpose()?;
        let mut response = mock_exchange::execute(deps.branch(), env, info, msg)
            .map_err(|e| StdError::generic_err(e.to_string()))?;
        if mode == Fault::Omit {
            response.messages.pop();
        } else if mode == Fault::Duplicate {
            response.messages.push(response.messages[1].clone());
        } else if mode == Fault::BankFailure {
            if let CosmosMsg::Bank(BankMsg::Send { amount, .. }) = &mut response.messages[0].msg {
                amount[0].amount = Uint128::MAX;
            }
        } else if mode != Fault::None {
            if let CosmosMsg::Wasm(WasmMsg::Execute {
                msg, contract_addr, ..
            }) = &mut response.messages[1].msg
            {
                let dca_example_protocol::DcaCallbackMsg::SwapResult { mut receipt } =
                    from_json(msg.clone())?;
                match mode {
                    Fault::Stale => receipt.correlation.wait_sequence += 1,
                    Fault::WrongOrder => receipt.order_id += 1,
                    Fault::WrongSlice => receipt.slice_index += 1,
                    Fault::WrongTask => receipt.correlation.workflow_id += 1,
                    Fault::InputAmount => receipt.input.amount += Uint128::one(),
                    Fault::InputDenom => receipt.input.denom = "uwrong".into(),
                    Fault::OutputDenom => receipt.output.denom = "uwrong".into(),
                    Fault::Beneficiary => receipt.beneficiary = proxy.clone().unwrap(),
                    Fault::BelowMinimum => receipt.output.amount = Uint128::one(),
                    _ => {}
                }
                *msg =
                    to_json_binary(&dca_example_protocol::DcaCallbackMsg::SwapResult { receipt })?;
                if mode == Fault::WrongSender {
                    *msg = to_json_binary(&Relay {
                        target: contract_addr.clone(),
                        msg: msg.clone(),
                    })?;
                    *contract_addr =
                        proxy.ok_or_else(|| StdError::generic_err("missing fault relay"))?;
                }
            }
        }
        Ok(response)
    }
    Box::new(ContractWrapper::new_with_empty(
        execute,
        mock_exchange::instantiate,
        mock_exchange::query,
    ))
}

pub fn faulty_dca() -> Box<dyn Contract<RuntimeMsg, RuntimeQuery>> {
    fn execute(
        mut deps: DepsMut<RuntimeQuery>,
        env: Env,
        info: MessageInfo,
        msg: native_dca::msg::ExecuteMsg,
    ) -> Result<Response<RuntimeMsg>, native_dca::error::ContractError> {
        let mode = fault(deps.storage)?;
        let callback = matches!(&msg, native_dca::msg::ExecuteMsg::SwapResult { .. });
        let mut response = native_dca::execute(deps.branch(), env, info, msg)?;
        if callback {
            for message in &mut response.messages {
                if let CosmosMsg::Custom(RuntimeMsg::Deliver {
                    wait_sequence,
                    resolution,
                    ..
                }) = &mut message.msg
                {
                    if mode == Fault::DeliveryFailure {
                        *wait_sequence += 1;
                    }
                    if mode == Fault::RemoteError {
                        *resolution = Resolution::Error {
                            code: "\"\\".repeat(128),
                            message: "\"\\".repeat(512),
                        };
                    }
                    if let Resolution::Success { data } = resolution {
                        let size = match mode {
                            Fault::Pad2048 => 2048,
                            Fault::Pad2049 => 2049,
                            _ => data.len(),
                        };
                        let mut bytes = data.to_vec();
                        bytes.resize(size, b' ');
                        *data = bytes.into();
                    }
                }
            }
        }
        Ok(response)
    }
    fn reply(
        mut deps: DepsMut<RuntimeQuery>,
        env: Env,
        mut msg: Reply,
    ) -> Result<Response<RuntimeMsg>, native_dca::error::ContractError> {
        let mode = fault(deps.storage)?;
        if msg.id == 2 && mode == Fault::ReplyFailure {
            msg.payload = Binary::from(b"{}".as_slice());
        }
        if msg.id == 2 && mode == Fault::ReplyCrossOrder {
            let mut payload: native_dca::msg::DispatchReplyPayload = from_json(&msg.payload)?;
            payload.order_id += 1;
            msg.payload = to_json_binary(&payload)?;
        }
        if msg.id == 2 && mode == Fault::ReplyReplay {
            native_dca::reply(deps.branch(), env.clone(), msg.clone())?;
        }
        native_dca::reply(deps, env, msg)
    }
    Box::new(
        ContractWrapper::new(execute, native_dca::instantiate, native_dca::query)
            .with_reply(reply)
            .with_sudo_empty(native_dca::sudo),
    )
}

#[derive(Serialize, Deserialize)]
struct Relay {
    target: String,
    msg: Binary,
}
pub fn relay() -> Box<dyn Contract<RuntimeMsg, RuntimeQuery>> {
    fn instantiate(
        _: DepsMut<RuntimeQuery>,
        _: Env,
        _: MessageInfo,
        _: Empty,
    ) -> StdResult<Response<RuntimeMsg>> {
        Ok(Response::new())
    }
    fn execute(
        _: DepsMut<RuntimeQuery>,
        _: Env,
        _: MessageInfo,
        msg: Relay,
    ) -> StdResult<Response<RuntimeMsg>> {
        Ok(Response::new().add_message(WasmMsg::Execute {
            contract_addr: msg.target,
            msg: msg.msg,
            funds: vec![],
        }))
    }
    fn query(_: Deps<RuntimeQuery>, _: Env, _: Empty) -> StdResult<Binary> {
        to_json_binary(&())
    }
    Box::new(ContractWrapper::new(execute, instantiate, query))
}

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
