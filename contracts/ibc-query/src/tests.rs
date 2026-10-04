use super::*;
use cosmwasm_std::testing::{
    message_info, mock_dependencies, mock_env, MockApi, MockQuerier, MockStorage,
};
type DepsT = OwnedDeps<MockStorage, MockApi, MockQuerier>;
fn setup() -> (DepsT, Env, Addr, IbcChannel) {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let owner = deps.api.addr_make("owner");
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&owner, &[]),
        InstantiateMsg {
            connection_id: "connection-0".into(),
            counterparty_port: "wasm.remote".into(),
            timeout_seconds: 60,
        },
    )
    .unwrap();
    let channel = IbcChannel::new(
        IbcEndpoint {
            port_id: format!("wasm.{}", env.contract.address),
            channel_id: "channel-0".into(),
        },
        IbcEndpoint {
            port_id: "wasm.remote".into(),
            channel_id: "channel-7".into(),
        },
        IbcOrder::Unordered,
        VERSION,
        "connection-0",
    );
    ibc_channel_open(
        deps.as_mut(),
        env.clone(),
        IbcChannelOpenMsg::new_init(channel.clone()),
    )
    .unwrap();
    ibc_channel_connect(
        deps.as_mut(),
        env.clone(),
        IbcChannelConnectMsg::new_ack(channel.clone(), VERSION),
    )
    .unwrap();
    (deps, env, owner, channel)
}
fn packet(response: &Response, channel: &IbcChannel, seq: u64) -> IbcPacket {
    match &response.messages[0].msg {
        CosmosMsg::Ibc(IbcMsg::SendPacket {
            channel_id,
            data,
            timeout,
        }) => {
            assert_eq!(channel_id, &channel.endpoint.channel_id);
            IbcPacket::new(
                data.clone(),
                channel.endpoint.clone(),
                channel.counterparty_endpoint.clone(),
                seq,
                timeout.clone(),
            )
        }
        other => panic!("unexpected {other:?}"),
    }
}
fn begin(deps: &mut DepsT, env: &Env, owner: &Addr, channel: &IbcChannel, value: u64) -> IbcPacket {
    let r = execute(
        deps.as_mut(),
        env.clone(),
        message_info(owner, &[]),
        ExecuteMsg::Start { value },
    )
    .unwrap();
    packet(&r, channel, 1)
}
fn ack(deps: &mut DepsT, env: &Env, p: IbcPacket, a: Ack) -> IbcBasicResponse {
    ibc_packet_ack(
        deps.as_mut(),
        env.clone(),
        IbcPacketAckMsg::new(
            IbcAcknowledgement::new(to_json_binary(&a).unwrap()),
            p,
            Addr::unchecked("relayer"),
        ),
    )
    .unwrap()
}
fn dispatch(deps: &mut DepsT, env: &Env, r: IbcBasicResponse) -> Response {
    assert_eq!(r.messages.len(), 1);
    match &r.messages[0].msg {
        CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr,
            msg,
            funds,
        }) => {
            assert_eq!(contract_addr, env.contract.address.as_str());
            assert!(funds.is_empty());
            execute(
                deps.as_mut(),
                env.clone(),
                message_info(&env.contract.address, &[]),
                from_json(msg).unwrap(),
            )
            .unwrap()
        }
        other => panic!("unexpected {other:?}"),
    }
}
fn status(deps: &DepsT) -> Status {
    load_instance(&deps.storage, 1).unwrap().status
}
fn failure(deps: &DepsT) -> WorkflowError {
    match status(deps) {
        Status::Failed { error } => from_json(error).unwrap(),
        s => panic!("{s:?}"),
    }
}
#[test]
fn two_waits_and_replay_do_not_skip_continuations() {
    let (mut deps, env, owner, ch) = setup();
    let first = begin(&mut deps, &env, &owner, &ch, 5);
    let r = ack(&mut deps, &env, first.clone(), Ack::Result { value: 10 });
    let next = dispatch(&mut deps, &env, r);
    assert!(ack(&mut deps, &env, first, Ack::Result { value: 10 })
        .messages
        .is_empty());
    let second = packet(&next, &ch, 2);
    let decoded: Packet = from_json(&second.data).unwrap();
    assert_eq!(decoded.correlation.wait_sequence, 2);
    assert_eq!(decoded.value, 10);
    let r = ack(&mut deps, &env, second.clone(), Ack::Result { value: 20 });
    dispatch(&mut deps, &env, r);
    match status(&deps) {
        Status::Completed { output } => assert_eq!(from_json::<u64>(output).unwrap(), 20),
        s => panic!("{s:?}"),
    }
    assert!(ack(&mut deps, &env, second, Ack::Result { value: 20 })
        .messages
        .is_empty());
    assert_eq!(
        durable_runtime::active_count(&deps.storage, "double_twice", 1).unwrap(),
        0
    );
}
#[test]
fn public_execute_cannot_forge_delivery_and_start_is_owner_only() {
    let (mut deps, env, owner, ch) = setup();
    begin(&mut deps, &env, &owner, &ch, 5);
    let attacker = deps.api.addr_make("attacker");
    assert!(execute(
        deps.as_mut(),
        env.clone(),
        message_info(&attacker, &[]),
        ExecuteMsg::Deliver {
            correlation: Correlation {
                workflow_id: 1,
                wait_sequence: 1
            },
            outcome: Outcome::Success(to_json_binary(&10u64).unwrap())
        }
    )
    .is_err());
    assert!(execute(
        deps.as_mut(),
        env,
        message_info(&attacker, &[]),
        ExecuteMsg::Start { value: 1 }
    )
    .is_err());
    assert!(matches!(status(&deps), Status::Waiting { .. }));
}
#[test]
fn error_and_malformed_ack_terminate_as_remote_errors() {
    for bytes in [
        to_json_binary(&Ack::Error {
            code: "overflow".into(),
        })
        .unwrap(),
        Binary::from(b"not-json".as_slice()),
        Binary::from(vec![b'x'; MAX_ACK + 1]),
        Binary::from(br#"{"result":{"value":7,"extra":1}}"#.as_slice()),
    ] {
        let (mut deps, env, owner, ch) = setup();
        let p = begin(&mut deps, &env, &owner, &ch, 5);
        let r = ibc_packet_ack(
            deps.as_mut(),
            env.clone(),
            IbcPacketAckMsg::new(
                IbcAcknowledgement::new(bytes),
                p,
                Addr::unchecked("relayer"),
            ),
        )
        .unwrap();
        dispatch(&mut deps, &env, r);
        assert!(matches!(
            failure(&deps),
            WorkflowError::Wait(WaitError::Remote { .. })
        ));
    }
}
#[test]
fn changed_packet_or_endpoint_cannot_settle_wait() {
    for mutation in 0..4 {
        let (mut deps, env, owner, ch) = setup();
        let mut p = begin(&mut deps, &env, &owner, &ch, 5);
        match mutation {
            0 => p.src.channel_id = "channel-99".into(),
            1 => p.dest.port_id = "wasm.attacker".into(),
            2 => {
                p.data = to_json_binary(&Packet {
                    correlation: Correlation {
                        workflow_id: 1,
                        wait_sequence: 1,
                    },
                    value: 999,
                })
                .unwrap()
            }
            _ => p.timeout = IbcTimeout::with_timestamp(env.block.time.plus_seconds(90)),
        }
        assert!(ibc_packet_ack(
            deps.as_mut(),
            env.clone(),
            IbcPacketAckMsg::new(
                IbcAcknowledgement::new(to_json_binary(&Ack::Result { value: 10 }).unwrap()),
                p,
                Addr::unchecked("relayer")
            )
        )
        .is_err());
        assert!(matches!(status(&deps), Status::Waiting { .. }));
    }
}
#[test]
fn timeout_and_ack_at_deadline_produce_timeout() {
    for use_ack in [false, true] {
        let (mut deps, mut env, owner, ch) = setup();
        let p = begin(&mut deps, &env, &owner, &ch, 5);
        env.block.time = Timestamp::from_seconds(env.block.time.seconds() + 60);
        let r = if use_ack {
            ack(&mut deps, &env, p, Ack::Result { value: 10 })
        } else {
            ibc_packet_timeout(
                deps.as_mut(),
                env.clone(),
                IbcPacketTimeoutMsg::new(p, Addr::unchecked("relayer")),
            )
            .unwrap()
        };
        dispatch(&mut deps, &env, r);
        assert!(matches!(
            failure(&deps),
            WorkflowError::Wait(WaitError::Timeout)
        ));
    }
}
#[test]
fn early_remote_timeout_waits_for_local_deadline() {
    let (mut deps, mut env, owner, ch) = setup();
    let p = begin(&mut deps, &env, &owner, &ch, 5);
    let r = ibc_packet_timeout(
        deps.as_mut(),
        env.clone(),
        IbcPacketTimeoutMsg::new(p, Addr::unchecked("relayer")),
    )
    .unwrap();
    assert!(r.messages.is_empty());
    assert!(matches!(status(&deps), Status::Waiting { .. }));
    assert!(execute(
        deps.as_mut(),
        env.clone(),
        message_info(&owner, &[]),
        ExecuteMsg::Expire {
            workflow_id: 1,
            wait_sequence: 1
        }
    )
    .is_err());
    env.block.time = env.block.time.plus_seconds(60);
    execute(
        deps.as_mut(),
        env,
        message_info(&owner, &[]),
        ExecuteMsg::Expire {
            workflow_id: 1,
            wait_sequence: 1,
        },
    )
    .unwrap();
    assert!(matches!(
        failure(&deps),
        WorkflowError::Wait(WaitError::Timeout)
    ));
}
#[test]
fn late_ack_after_permissionless_expiry_cannot_reactivate() {
    let (mut deps, mut env, owner, ch) = setup();
    let p = begin(&mut deps, &env, &owner, &ch, 5);
    env.block.time = env.block.time.plus_seconds(60);
    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&owner, &[]),
        ExecuteMsg::Expire {
            workflow_id: 1,
            wait_sequence: 1,
        },
    )
    .unwrap();
    assert!(ack(&mut deps, &env, p, Ack::Result { value: 10 })
        .messages
        .is_empty());
    assert!(matches!(
        failure(&deps),
        WorkflowError::Wait(WaitError::Timeout)
    ));
}
#[test]
fn closed_channel_fails_pending_result_without_dispatching_another_packet() {
    let (mut deps, env, owner, ch) = setup();
    let p = begin(&mut deps, &env, &owner, &ch, 5);
    ibc_channel_close(
        deps.as_mut(),
        env.clone(),
        IbcChannelCloseMsg::new_confirm(ch),
    )
    .unwrap();
    let r = ack(&mut deps, &env, p, Ack::Result { value: 10 });
    assert!(dispatch(&mut deps, &env, r).messages.is_empty());
    assert!(
        matches!(failure(&deps), WorkflowError::Wait(WaitError::Remote { code, .. }) if code == "channel_closed")
    );
}
#[test]
fn handshake_rejects_wrong_trust_parameters_and_rebinding() {
    let (mut deps, env, _, ch) = setup();
    assert!(ibc_channel_connect(
        deps.as_mut(),
        env.clone(),
        IbcChannelConnectMsg::new_ack(ch.clone(), VERSION)
    )
    .is_err());
    for mutation in 0..5 {
        let mut bad = ch.clone();
        match mutation {
            0 => bad.order = IbcOrder::Ordered,
            1 => bad.version = "other".into(),
            2 => bad.connection_id = "connection-99".into(),
            3 => bad.counterparty_endpoint.port_id = "wasm.attacker".into(),
            _ => bad.endpoint.port_id = "wasm.other".into(),
        }
        assert!(validate_channel(deps.as_ref(), &env, &bad, Some(VERSION)).is_err());
    }
    assert!(validate_channel(deps.as_ref(), &env, &ch, Some("bad-version")).is_err());
}
#[test]
fn receiver_acknowledges_success_overflow_and_invalid_application_data() {
    let (mut deps, env, _, ch) = setup();
    for (data, expected) in [
        (
            to_json_binary(&Packet {
                correlation: Correlation {
                    workflow_id: 3,
                    wait_sequence: 2,
                },
                value: 5,
            })
            .unwrap(),
            Ack::Result { value: 10 },
        ),
        (
            to_json_binary(&Packet {
                correlation: Correlation {
                    workflow_id: 3,
                    wait_sequence: 2,
                },
                value: u64::MAX,
            })
            .unwrap(),
            Ack::Error {
                code: "overflow".into(),
            },
        ),
        (
            Binary::from(vec![b'x'; MAX_PACKET + 1]),
            Ack::Error {
                code: "bad_request".into(),
            },
        ),
    ] {
        let p = IbcPacket::new(
            data,
            ch.counterparty_endpoint.clone(),
            ch.endpoint.clone(),
            1,
            IbcTimeout::with_timestamp(env.block.time.plus_seconds(60)),
        );
        let r = ibc_packet_receive(
            deps.as_mut(),
            env.clone(),
            IbcPacketReceiveMsg::new(p, Addr::unchecked("relayer")),
        )
        .unwrap();
        assert_eq!(
            from_json::<Ack>(r.acknowledgement.unwrap()).unwrap(),
            expected
        );
    }
}
