//! These tests execute real Wasm with mock host storage/API. They do not dispatch
//! response messages or provide the Cosmos SDK transaction cache/rollback layer.
use cosmwasm_std::{Binary, Empty, Env, Order};
use cosmwasm_vm::internals::{check_wasm, Logger};
use cosmwasm_vm::testing::{mock_backend, mock_env, mock_info, MockApi, MockQuerier, MockStorage};
use cosmwasm_vm::{
    call_execute, call_instantiate, call_query, capabilities_from_csv, Instance, InstanceOptions,
    Size, Storage, VmError, WasmLimits,
};
use serde_json::{json, Value};

type Vm = Instance<MockApi, MockStorage, MockQuerier>;
const GAS: u64 = 20_000_000_000;
fn wasm() -> Vec<u8> {
    let path = std::env::var("DURABLE_WASM")
        .expect("run scripts/check-wasm.sh; DURABLE_WASM must identify the compiled artifact");
    std::fs::read(path).unwrap()
}
fn vm() -> Vm {
    let wasm = wasm();
    // This is the same compatibility validator used by cosmwasm-check 2.2.2.
    check_wasm(
        &wasm,
        &capabilities_from_csv("iterator,staking,cosmwasm_1_1,cosmwasm_1_2,cosmwasm_1_3,cosmwasm_1_4,cosmwasm_2_0,cosmwasm_2_1,cosmwasm_2_2"),
        &WasmLimits::default(), Logger::Off,
    ).unwrap();
    Instance::from_code(
        &wasm,
        mock_backend(&[]),
        InstanceOptions { gas_limit: GAS },
        Some(Size::mebi(32)),
    )
    .unwrap()
}
fn fresh(vm: Vm, gas: u64) -> Vm {
    // Preserve only the host backend. Every invocation gets new Wasm memory and gas.
    Instance::from_code(
        &wasm(),
        vm.recycle().unwrap(),
        InstanceOptions { gas_limit: gas },
        Some(Size::mebi(32)),
    )
    .unwrap()
}
fn addr(name: &str) -> String {
    MockApi::default().addr_make(name)
}
fn execute(vm: &mut Vm, env: &Env, sender: &str, msg: Value) -> cosmwasm_std::Response {
    call_execute::<_, _, _, Empty>(
        vm,
        env,
        &mock_info(&addr(sender), &[]),
        &serde_json::to_vec(&msg).unwrap(),
    )
    .unwrap()
    .unwrap()
}
fn query(vm: &mut Vm, env: &Env, msg: Value) -> Value {
    let bytes = call_query(vm, env, &serde_json::to_vec(&msg).unwrap())
        .unwrap()
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
fn instance(vm: &mut Vm, env: &Env) -> Value {
    query(vm, env, json!({"Instance":{"workflow_id":1}}))
}
fn initialized() -> (Vm, Env) {
    let mut vm = vm();
    let env = mock_env();
    call_instantiate::<_, _, _, Empty>(&mut vm, &env, &mock_info(&addr("owner"), &[]), &serde_json::to_vec(&json!({"payment_service":addr("payment"),"shipment_service":addr("shipment"),"deadline_blocks":100})).unwrap()).unwrap().unwrap();
    (fresh(vm, GAS), env)
}
fn success(sequence: u64, output: Value) -> Value {
    json!({"Resume":{"workflow_id":1,"wait_sequence":sequence,"outcome":{"Success":Binary::from(serde_json::to_vec(&output).unwrap())}}})
}
fn snapshot(vm: &mut Vm) -> Vec<(Vec<u8>, Vec<u8>)> {
    vm.with_storage(|s| {
        let id = s.scan(None, None, Order::Ascending).0.unwrap();
        Ok(s.all(id).0.unwrap())
    })
    .unwrap()
}

#[test]
fn compiled_workflow_restores_state_across_fresh_invocations() {
    let (mut vm, mut env) = initialized();
    let start = execute(&mut vm, &env, "owner", json!({"Start":{"order":{"id":42}}}));
    assert_eq!(start.messages.len(), 1); // Emitted only; the host must dispatch it.
    assert_eq!(start.data.unwrap().as_slice(), b"1");
    assert_eq!(
        instance(&mut vm, &env)["status"]["Waiting"]["wait"]["sequence"],
        1
    );
    vm = fresh(vm, GAS);
    env.block.height += 1;
    let payment = execute(
        &mut vm,
        &env,
        "payment",
        success(1, json!({"reference":"paid"})),
    );
    assert_eq!(payment.messages.len(), 1);
    assert_eq!(
        instance(&mut vm, &env)["status"]["Waiting"]["wait"]["sequence"],
        2
    );
    assert_eq!(
        query(&mut vm, &env, json!({"PaymentSeen":{"order_id":42}})),
        json!({"reference":"paid"})
    );
    vm = fresh(vm, GAS);
    env.block.height += 1;
    execute(
        &mut vm,
        &env,
        "shipment",
        success(2, json!({"tracking":"tracking-42"})),
    );
    let output: Binary =
        serde_json::from_value(instance(&mut vm, &env)["status"]["Completed"]["output"].clone())
            .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&output).unwrap(),
        json!({"order_id":42,"shipment":{"tracking":"tracking-42"}})
    );
    assert_eq!(query(&mut vm, &env, json!({"ActiveCount":{}})), 0);
}

#[test]
fn unauthorized_and_stale_callbacks_do_not_mutate_storage() {
    let (mut vm, env) = initialized();
    execute(&mut vm, &env, "owner", json!({"Start":{"order":{"id":42}}}));
    vm = fresh(vm, GAS);
    for (sender, sequence) in [("intruder", 1), ("payment", 2)] {
        let before = snapshot(&mut vm);
        let result = call_execute::<_, _, _, Empty>(
            &mut vm,
            &env,
            &mock_info(&addr(sender), &[]),
            &serde_json::to_vec(&success(sequence, json!({"reference":"paid"}))).unwrap(),
        )
        .unwrap();
        assert!(result.is_err());
        assert_eq!(snapshot(&mut vm), before);
        vm = fresh(vm, GAS);
    }
}

#[test]
fn deadline_boundary_and_application_failure_execute_in_wasm() {
    let (mut vm, mut env) = initialized();
    execute(&mut vm, &env, "owner", json!({"Start":{"order":{"id":42}}}));
    vm = fresh(vm, GAS);
    env.block.height += 100;
    let before = snapshot(&mut vm);
    assert!(call_execute::<_, _, _, Empty>(
        &mut vm,
        &env,
        &mock_info(&addr("payment"), &[]),
        &serde_json::to_vec(&success(1, json!({"reference":"paid"}))).unwrap()
    )
    .unwrap()
    .is_err());
    assert_eq!(snapshot(&mut vm), before);
    vm = fresh(vm, GAS);
    execute(
        &mut vm,
        &env,
        "anyone",
        json!({"Expire":{"workflow_id":1,"wait_sequence":1}}),
    );
    assert!(instance(&mut vm, &env)["status"].get("Failed").is_some());
    assert_eq!(query(&mut vm, &env, json!({"ActiveCount":{}})), 0);

    let (mut vm, env) = initialized();
    execute(&mut vm, &env, "owner", json!({"Start":{"order":{"id":42}}}));
    vm = fresh(vm, GAS);
    execute(
        &mut vm,
        &env,
        "payment",
        success(1, json!({"reference":""})),
    );
    assert!(instance(&mut vm, &env)["status"].get("Failed").is_some());
    assert_eq!(
        query(&mut vm, &env, json!({"PaymentSeen":{"order_id":42}})),
        json!({"reference":""})
    );
}

#[test]
fn callback_exhausts_vm_gas_when_budget_is_insufficient() {
    let (mut vm, env) = initialized();
    execute(&mut vm, &env, "owner", json!({"Start":{"order":{"id":42}}}));
    vm = fresh(vm, 1);
    let result = call_execute::<_, _, _, Empty>(
        &mut vm,
        &env,
        &mock_info(&addr("payment"), &[]),
        &serde_json::to_vec(&success(1, json!({"reference":"paid"}))).unwrap(),
    );
    assert!(
        matches!(result, Err(VmError::GasDepletion { .. })),
        "{result:?}"
    );
    // Deliberately discard the failed backend: VM alone does not provide rollback.
}

#[test]
fn report_callback_payload_costs() {
    // Payload sweep, NOT a checkpoint-size benchmark (the saved Order is fixed).
    for bytes in [1, 1024, 8192] {
        let (mut vm, env) = initialized();
        execute(&mut vm, &env, "owner", json!({"Start":{"order":{"id":42}}}));
        vm = fresh(vm, GAS);
        execute(
            &mut vm,
            &env,
            "payment",
            success(1, json!({"reference":"p".repeat(bytes)})),
        );
        let gas = vm.create_gas_report();
        let stored_bytes: usize = snapshot(&mut vm)
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum();
        println!("VM_METRIC payload_bytes={bytes} internal_gas={} mock_host_gas={} stored_key_value_bytes={stored_bytes}", gas.used_internally, gas.used_externally);
    }
}
