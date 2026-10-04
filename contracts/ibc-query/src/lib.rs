//! Two durable waits over IBC classic. No tokens or external side effects.
use cosmwasm_std::*;
use durable_macro::durable_workflow;
use durable_runtime::{
    expire, load_instance, registration, resume, start, Correlation, Deadline, Limits, Operation,
    Outcome, PreparedWait, Registry, RuntimeError, RuntimeResult, Status, WaitError, WorkflowCtx,
};
use serde::{Deserialize, Serialize};

const CONFIG: &[u8] = b"ibc-demo/config";
const CHANNEL: &[u8] = b"ibc-demo/channel";
const CLOSED: &[u8] = b"ibc-demo/closed";
pub const VERSION: &str = "durable-query-1";
const MAX_PACKET: usize = 1024;
const MAX_ACK: usize = 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstantiateMsg {
    /// An already established IBC connection. Immutable trust anchor.
    pub connection_id: String,
    pub counterparty_port: String,
    pub timeout_seconds: u64,
}
#[derive(Clone, Serialize, Deserialize)]
struct Config {
    owner: Addr,
    connection_id: String,
    counterparty_port: String,
    timeout_seconds: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecuteMsg {
    Start {
        value: u64,
    },
    Expire {
        workflow_id: u64,
        wait_sequence: u64,
    },
    /// Internal only; emitted by authenticated IBC entry points.
    Deliver {
        correlation: Correlation,
        outcome: Outcome,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryMsg {
    Instance { workflow_id: u64 },
    Channel {},
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packet {
    pub correlation: Correlation,
    pub value: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Ack {
    Result { value: u64 },
    Error { code: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pending {
    data: Binary,
    src: IbcEndpoint,
    dest: IbcEndpoint,
    timeout: IbcTimeout,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub value: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, thiserror::Error)]
pub enum WorkflowError {
    #[error("{0}")]
    Wait(#[from] WaitError),
}

fn err(message: &str) -> StdError {
    StdError::generic_err(message)
}
fn get<T: serde::de::DeserializeOwned>(storage: &dyn Storage, key: &[u8]) -> StdResult<T> {
    from_json(
        storage
            .get(key)
            .ok_or_else(|| err("missing configuration or channel"))?,
    )
}
fn pending_key(key: Correlation) -> Vec<u8> {
    format!("ibc-demo/pending/{}/{}", key.workflow_id, key.wait_sequence).into_bytes()
}
fn registry() -> RuntimeResult<Registry> {
    Registry::new(vec![registration::<double_twice::Workflow>()])
}
fn runtime(e: RuntimeError) -> StdError {
    err(&e.to_string())
}

pub struct RemoteDouble;
impl Operation for RemoteDouble {
    const KIND: &'static str = "ibc_double";
    type Request = Request;
    type Output = u64;
    fn prepare(
        ctx: &mut WorkflowCtx<'_>,
        key: Correlation,
        request: Request,
    ) -> RuntimeResult<PreparedWait> {
        let cfg: Config = get(ctx.storage(), CONFIG)?;
        let channel: IbcChannel = get(ctx.storage(), CHANNEL)?;
        if ctx.storage().get(CLOSED).is_some() {
            return Err(RuntimeError::Validation("channel closed".into()));
        }
        let deadline = ctx
            .env()
            .block
            .time
            .seconds()
            .checked_add(cfg.timeout_seconds)
            .ok_or_else(|| RuntimeError::Validation("deadline overflow".into()))?;
        let timeout = IbcTimeout::with_timestamp(Timestamp::from_seconds(deadline));
        let data = to_json_binary(&Packet {
            correlation: key,
            value: request.value,
        })?;
        let pending = Pending {
            data: data.clone(),
            src: channel.endpoint.clone(),
            dest: channel.counterparty_endpoint,
            timeout: timeout.clone(),
        };
        ctx.storage()
            .set(&pending_key(key), &to_json_binary(&pending)?);
        Ok(PreparedWait {
            operation: Self::KIND.into(),
            request: data.clone(),
            resolver: ctx.env().contract.address.clone(),
            deadline: Deadline::Time(deadline),
            messages: vec![IbcMsg::SendPacket {
                channel_id: channel.endpoint.channel_id,
                data,
                timeout,
            }
            .into()],
        })
    }
}

#[durable_workflow(kind = "double_twice", version = 1)]
pub async fn double_twice(ctx: WorkflowCtx, value: u64) -> Result<u64, WorkflowError> {
    let doubled: u64 = ctx
        .wait::<RemoteDouble>(Request { value })
        .checkpoint()
        .await?;
    let result: u64 = ctx
        .wait::<RemoteDouble>(Request { value: doubled })
        .checkpoint()
        .await?;
    Ok(result)
}

#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    _: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> StdResult<Response> {
    if !info.funds.is_empty() {
        return Err(err("no funds accepted"));
    }
    if !msg.connection_id.starts_with("connection-")
        || !msg.counterparty_port.starts_with("wasm.")
        || msg.counterparty_port.len() > 128
        || !(1..=86400).contains(&msg.timeout_seconds)
    {
        return Err(err("invalid IBC configuration"));
    }
    deps.storage.set(
        CONFIG,
        &to_json_binary(&Config {
            owner: info.sender,
            connection_id: msg.connection_id,
            counterparty_port: msg.counterparty_port,
            timeout_seconds: msg.timeout_seconds,
        })?,
    );
    Ok(Response::new())
}

#[entry_point]
pub fn execute(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> StdResult<Response> {
    if !info.funds.is_empty() {
        return Err(err("no funds accepted"));
    }
    match msg {
        ExecuteMsg::Start { value } => {
            if get::<Config>(deps.storage, CONFIG)?.owner != info.sender {
                return Err(err("owner only"));
            }
            let (id, response) = start::<double_twice::Workflow>(
                deps,
                env,
                info,
                double_twice::Input { value },
                &Limits::default(),
            )
            .map_err(runtime)?;
            Ok(response.set_data(to_json_binary(&id)?))
        }
        ExecuteMsg::Expire {
            workflow_id,
            wait_sequence,
        } => {
            let response = expire(
                deps.branch(),
                env,
                info,
                workflow_id,
                wait_sequence,
                &registry().map_err(runtime)?,
                &Limits::default(),
            )
            .map_err(runtime)?;
            deps.storage.remove(&pending_key(Correlation {
                workflow_id,
                wait_sequence,
            }));
            Ok(response)
        }
        ExecuteMsg::Deliver {
            correlation,
            outcome,
        } => {
            if info.sender != env.contract.address {
                return Err(err("self only"));
            }
            resume(
                deps,
                env,
                info,
                correlation.workflow_id,
                correlation.wait_sequence,
                outcome,
                &registry().map_err(runtime)?,
                &Limits::default(),
            )
            .map_err(runtime)
        }
    }
}
#[entry_point]
pub fn query(deps: Deps, _: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Instance { workflow_id } => {
            to_json_binary(&load_instance(deps.storage, workflow_id).map_err(runtime)?)
        }
        QueryMsg::Channel {} => to_json_binary(&(
            get::<IbcChannel>(deps.storage, CHANNEL)?,
            deps.storage.get(CLOSED).is_some(),
        )),
    }
}

fn validate_channel(
    deps: Deps,
    env: &Env,
    channel: &IbcChannel,
    peer_version: Option<&str>,
) -> StdResult<()> {
    let cfg: Config = get(deps.storage, CONFIG)?;
    if channel.order != IbcOrder::Unordered
        || channel.version != VERSION
        || peer_version.is_some_and(|v| v != VERSION)
        || channel.connection_id != cfg.connection_id
        || channel.counterparty_endpoint.port_id != cfg.counterparty_port
        || channel.endpoint.port_id != format!("wasm.{}", env.contract.address)
    {
        return Err(err("untrusted channel parameters"));
    }
    Ok(())
}
#[entry_point]
pub fn ibc_channel_open(
    deps: DepsMut,
    env: Env,
    msg: IbcChannelOpenMsg,
) -> StdResult<IbcChannelOpenResponse> {
    if deps.storage.get(CHANNEL).is_some() {
        return Err(err("channel already bound"));
    }
    validate_channel(
        deps.as_ref(),
        &env,
        msg.channel(),
        msg.counterparty_version(),
    )?;
    Ok(Some(Ibc3ChannelOpenResponse {
        version: VERSION.into(),
    }))
}
#[entry_point]
pub fn ibc_channel_connect(
    deps: DepsMut,
    env: Env,
    msg: IbcChannelConnectMsg,
) -> StdResult<IbcBasicResponse> {
    if deps.storage.get(CHANNEL).is_some() {
        return Err(err("channel already bound"));
    }
    validate_channel(
        deps.as_ref(),
        &env,
        msg.channel(),
        msg.counterparty_version(),
    )?;
    deps.storage.set(CHANNEL, &to_json_binary(msg.channel())?);
    Ok(IbcBasicResponse::new())
}
#[entry_point]
pub fn ibc_channel_close(
    deps: DepsMut,
    _: Env,
    msg: IbcChannelCloseMsg,
) -> StdResult<IbcBasicResponse> {
    if get::<IbcChannel>(deps.storage, CHANNEL)? != *msg.channel() {
        return Err(err("wrong channel"));
    }
    deps.storage.set(CLOSED, b"1");
    // Outstanding work can still expire; retain endpoints for late acknowledgements.
    Ok(IbcBasicResponse::new().add_attribute("channel_closed", "true"))
}
#[entry_point]
pub fn ibc_packet_receive(
    deps: DepsMut,
    _: Env,
    msg: IbcPacketReceiveMsg,
) -> StdResult<IbcReceiveResponse> {
    let channel: IbcChannel = get(deps.storage, CHANNEL)?;
    if msg.packet.src != channel.counterparty_endpoint
        || msg.packet.dest != channel.endpoint
        || deps.storage.get(CLOSED).is_some()
    {
        return Err(err("wrong or closed channel"));
    }
    // Application errors are acknowledgements so the source can terminate.
    let ack = if msg.packet.data.len() > MAX_PACKET {
        Ack::Error {
            code: "bad_request".into(),
        }
    } else {
        match from_json::<Packet>(&msg.packet.data) {
            Ok(packet) => match packet.value.checked_mul(2) {
                Some(value) => Ack::Result { value },
                None => Ack::Error {
                    code: "overflow".into(),
                },
            },
            Err(_) => Ack::Error {
                code: "bad_request".into(),
            },
        }
    };
    Ok(IbcReceiveResponse::new(to_json_binary(&ack)?))
}

fn outcome(ack: Binary) -> Outcome {
    if ack.len() <= MAX_ACK {
        match from_json::<Ack>(&ack) {
            Ok(Ack::Result { value }) => {
                return Outcome::Success(to_json_binary(&value).expect("u64 JSON"))
            }
            Ok(Ack::Error { code }) if code.len() <= 64 => {
                return Outcome::Error {
                    code,
                    message: "remote query failed".into(),
                }
            }
            _ => {}
        }
    }
    Outcome::Error {
        code: "invalid_ack".into(),
        message: "peer returned malformed acknowledgement".into(),
    }
}

fn settle(
    deps: DepsMut,
    env: Env,
    packet: IbcPacket,
    result: Option<Outcome>,
) -> StdResult<IbcBasicResponse> {
    if packet.data.len() > MAX_PACKET {
        return Err(err("packet too large"));
    }
    let decoded: Packet = from_json(&packet.data)?;
    let key = pending_key(decoded.correlation);
    let channel: IbcChannel = get(deps.storage, CHANNEL)?;
    if packet.src != channel.endpoint || packet.dest != channel.counterparty_endpoint {
        return Err(err("wrong packet endpoints"));
    }
    let Some(encoded) = deps.storage.get(&key) else {
        // IBC core prevents proof replays; idempotent also at the adapter boundary.
        return Ok(IbcBasicResponse::new().add_attribute("ibc_resolution", "already_settled"));
    };
    let expected: Pending = from_json(encoded)?;
    if packet.data != expected.data
        || packet.src != expected.src
        || packet.dest != expected.dest
        || packet.timeout != expected.timeout
    {
        return Err(err("packet does not match pending intent"));
    }
    let instance = load_instance(deps.storage, decoded.correlation.workflow_id).map_err(runtime)?;
    let response = IbcBasicResponse::new();
    let message = match instance.status {
        Status::Waiting { wait, .. } if wait.sequence == decoded.correlation.wait_sequence => {
            if wait.deadline.reached(&env) {
                Some(ExecuteMsg::Expire {
                    workflow_id: decoded.correlation.workflow_id,
                    wait_sequence: decoded.correlation.wait_sequence,
                })
            } else if let Some(outcome) = result {
                let outcome = if deps.storage.get(CLOSED).is_some() {
                    Outcome::Error {
                        code: "channel_closed".into(),
                        message: "channel closed before continuation".into(),
                    }
                } else {
                    outcome
                };
                Some(ExecuteMsg::Deliver {
                    correlation: decoded.correlation,
                    outcome,
                })
            } else {
                // Counterparty clock can reach the packet timeout before this chain.
                // IBC has consumed the commitment; local expiration is still required.
                deps.storage.remove(&key);
                return Ok(
                    response.add_attribute("ibc_resolution", "timeout_before_local_deadline")
                );
            }
        }
        _ => None, // Late ack/timeout cannot reactivate a terminal workflow.
    };
    deps.storage.remove(&key);
    if let Some(message) = message {
        Ok(response.add_message(WasmMsg::Execute {
            contract_addr: env.contract.address.into_string(),
            msg: to_json_binary(&message)?,
            funds: vec![],
        }))
    } else {
        Ok(response.add_attribute("ibc_resolution", "late"))
    }
}
#[entry_point]
pub fn ibc_packet_ack(
    deps: DepsMut,
    env: Env,
    msg: IbcPacketAckMsg,
) -> StdResult<IbcBasicResponse> {
    settle(
        deps,
        env,
        msg.original_packet,
        Some(outcome(msg.acknowledgement.data)),
    )
}
#[entry_point]
pub fn ibc_packet_timeout(
    deps: DepsMut,
    env: Env,
    msg: IbcPacketTimeoutMsg,
) -> StdResult<IbcBasicResponse> {
    settle(deps, env, msg.packet, None)
}

#[cfg(test)]
mod tests;
