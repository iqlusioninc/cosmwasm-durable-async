//! Compiled IBC callback fixtures: two VMs, but no IBC proofs or SDK transactions.
use cosmwasm_std::*;
use cosmwasm_vm::internals::{check_wasm, Logger};
use cosmwasm_vm::testing::{mock_backend, mock_env, mock_info, MockApi, MockQuerier, MockStorage};
use cosmwasm_vm::{
    call_execute, call_ibc_channel_close, call_ibc_channel_connect, call_ibc_channel_open,
    call_ibc_packet_ack, call_ibc_packet_receive, call_ibc_packet_timeout, call_instantiate,
    call_query, capabilities_from_csv, Instance, InstanceOptions, Size, WasmLimits,
};
use serde_json::{json, Value};
type Vm = Instance<MockApi, MockStorage, MockQuerier>;
const GAS: u64 = 100_000_000_000;
fn vm() -> Vm {
    let wasm =
        std::fs::read(std::env::var("IBC_WASM").expect("run scripts/check-wasm.sh")).unwrap();
    check_wasm(&wasm, &capabilities_from_csv("iterator,staking,stargate,cosmwasm_1_1,cosmwasm_1_2,cosmwasm_1_3,cosmwasm_1_4,cosmwasm_2_0,cosmwasm_2_1,cosmwasm_2_2"), &WasmLimits::default(), Logger::Off).unwrap();
    Instance::from_code(
        &wasm,
        mock_backend(&[]),
        InstanceOptions { gas_limit: GAS },
        Some(Size::mebi(32)),
    )
    .unwrap()
}
fn addr(name: &str) -> String {
    MockApi::default().addr_make(name)
}
fn initialize(
    name: &str,
    peer: &str,
    local_channel: &str,
    remote_channel: &str,
) -> (Vm, Env, IbcChannel) {
    let mut vm = vm();
    let mut env = mock_env();
    env.contract.address = Addr::unchecked(addr(name));
    let ch = IbcChannel::new(
        IbcEndpoint {
            port_id: format!("wasm.{}", env.contract.address),
            channel_id: local_channel.into(),
        },
        IbcEndpoint {
            port_id: format!("wasm.{}", addr(peer)),
            channel_id: remote_channel.into(),
        },
        IbcOrder::Unordered,
        "durable-query-1",
        "connection-0",
    );
    call_instantiate::<_, _, _, Empty>(&mut vm, &env, &mock_info(&addr("owner"), &[]), &serde_json::to_vec(&json!({"connection_id":"connection-0", "counterparty_port":ch.counterparty_endpoint.port_id,"timeout_seconds":60})).unwrap()).unwrap().unwrap();
    call_ibc_channel_open(&mut vm, &env, &IbcChannelOpenMsg::new_init(ch.clone()))
        .unwrap()
        .unwrap();
    call_ibc_channel_connect::<_, _, _, Empty>(
        &mut vm,
        &env,
        &IbcChannelConnectMsg::new_ack(ch.clone(), "durable-query-1"),
    )
    .unwrap()
    .unwrap();
    (vm, env, ch)
}
fn execute(vm: &mut Vm, env: &Env, sender: &str, msg: Value) -> Response {
    call_execute(
        vm,
        env,
        &mock_info(sender, &[]),
        &serde_json::to_vec(&msg).unwrap(),
    )
    .unwrap()
    .unwrap()
}
fn packet(r: &Response, ch: &IbcChannel, seq: u64) -> IbcPacket {
    match &r.messages[0].msg {
        CosmosMsg::Ibc(IbcMsg::SendPacket { data, timeout, .. }) => IbcPacket::new(
            data.clone(),
            ch.endpoint.clone(),
            ch.counterparty_endpoint.clone(),
            seq,
            timeout.clone(),
        ),
        other => panic!("unexpected {other:?}"),
    }
}
fn deliver(vm: &mut Vm, env: &Env, r: IbcBasicResponse) -> Response {
    match &r.messages[0].msg {
        CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr,
            msg,
            funds,
        }) => {
            assert_eq!(contract_addr, env.contract.address.as_str());
            assert!(funds.is_empty());
            call_execute(
                vm,
                env,
                &mock_info(env.contract.address.as_str(), &[]),
                msg.as_slice(),
            )
            .unwrap()
            .unwrap()
        }
        other => panic!("unexpected {other:?}"),
    }
}
fn status(vm: &mut Vm, env: &Env) -> Value {
    let bytes = call_query(vm, env, br#"{"instance":{"workflow_id":1}}"#)
        .unwrap()
        .unwrap();
    serde_json::from_slice::<Value>(&bytes).unwrap()["status"].clone()
}
#[test]
fn compiled_ibc_two_wait_workflow_roundtrip_and_self_authentication() {
    let (mut a, env_a, ch) = initialize("a", "b", "channel-0", "channel-7");
    let (mut b, env_b, _) = initialize("b", "a", "channel-7", "channel-0");
    let mut r = execute(&mut a, &env_a, &addr("owner"), json!({"start":{"value":5}}));
    let forged = call_execute::<_, _, _, Empty>(&mut a, &env_a, &mock_info(&addr("attacker"), &[]), &serde_json::to_vec(&json!({"deliver":{"correlation":{"workflow_id":1,"wait_sequence":1},"outcome":{"Success":Binary::from(b"10".as_slice())}}})).unwrap()).unwrap();
    assert!(forged.is_err());
    for seq in 1..=2 {
        let p = packet(&r, &ch, seq);
        let received = call_ibc_packet_receive::<_, _, _, Empty>(
            &mut b,
            &env_b,
            &IbcPacketReceiveMsg::new(p.clone(), Addr::unchecked(addr("relayer"))),
        )
        .unwrap()
        .unwrap();
        let settled = call_ibc_packet_ack::<_, _, _, Empty>(
            &mut a,
            &env_a,
            &IbcPacketAckMsg::new(
                IbcAcknowledgement::new(received.acknowledgement.unwrap()),
                p,
                Addr::unchecked(addr("relayer")),
            ),
        )
        .unwrap()
        .unwrap();
        r = deliver(&mut a, &env_a, settled);
    }
    assert!(r.messages.is_empty());
    let s = status(&mut a, &env_a);
    assert_eq!(
        Binary::from_base64(s["Completed"]["output"].as_str().unwrap())
            .unwrap()
            .as_slice(),
        b"20"
    );
}
#[test]
fn compiled_ibc_timeout_and_channel_close_exports() {
    let (mut a, mut env, ch) = initialize("a", "b", "channel-0", "channel-7");
    let r = execute(&mut a, &env, &addr("owner"), json!({"start":{"value":5}}));
    let p = packet(&r, &ch, 1);
    env.block.time = env.block.time.plus_seconds(60);
    let timeout = call_ibc_packet_timeout::<_, _, _, Empty>(
        &mut a,
        &env,
        &IbcPacketTimeoutMsg::new(p, Addr::unchecked(addr("relayer"))),
    )
    .unwrap()
    .unwrap();
    deliver(&mut a, &env, timeout);
    assert!(status(&mut a, &env).get("Failed").is_some());
    call_ibc_channel_close::<_, _, _, Empty>(&mut a, &env, &IbcChannelCloseMsg::new_confirm(ch))
        .unwrap()
        .unwrap();
}
