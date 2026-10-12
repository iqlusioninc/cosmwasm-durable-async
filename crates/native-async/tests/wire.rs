use native_async::{PollResponse, RuntimeMsg, TaskResponse, TaskSudoMsg};
use serde::{de::DeserializeOwned, Serialize};

#[test]
fn funding_config_accepts_legacy_wire_without_price_observability() {
    let config: native_async::FundingConfig = serde_json::from_value(serde_json::json!({
        "denom": "ugas", "gas_price": "1", "max_gas_per_poll": 100,
        "max_lifetime_blocks": 100,
    }))
    .unwrap();
    assert_eq!(config.gas_price, cosmwasm_std::Decimal::one());
    assert_eq!(config.min_base_price, cosmwasm_std::Decimal::zero());
    assert_eq!(config.target_gas, 0);
    assert_eq!(config.price_grace_blocks, 0);
    assert_eq!(config.last_block_poll_gas_used, 0);
}

#[test]
fn go_funding_query_decimal_boundaries_decode_through_typed_rust_abi() {
    let cases: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../fixtures/native-async/decimal-domain.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        let funding = serde_json::from_value::<native_async::TaskFunding>(case["funding"].clone());
        assert_eq!(
            funding.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        if case["valid"] == true {
            assert_eq!(
                funding.unwrap().max_gas_price.atomics(),
                cosmwasm_std::Uint128::MAX
            );
        }
    }
}

fn roundtrip<T: DeserializeOwned + Serialize>(bytes: &[u8]) {
    let typed: T = serde_json::from_slice(bytes).unwrap();
    let original: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), original);
}

#[test]
fn shared_go_rust_wire_fixtures() {
    roundtrip::<TaskResponse>(include_bytes!(
        "../../../fixtures/native-async/v1/task-runnable.json"
    ));
    roundtrip::<TaskResponse>(include_bytes!(
        "../../../fixtures/native-async/v1/task-paused.json"
    ));
    roundtrip::<TaskResponse>(include_bytes!(
        "../../../fixtures/native-async/v1/task-completed.json"
    ));
    roundtrip::<TaskResponse>(include_bytes!(
        "../../../fixtures/native-async/v1/task-failed.json"
    ));
    roundtrip::<TaskResponse>(include_bytes!(
        "../../../fixtures/native-async/v1/task-expired.json"
    ));
    roundtrip::<RuntimeMsg>(include_bytes!(
        "../../../fixtures/native-async/v1/extend-lease.json"
    ));
    roundtrip::<RuntimeMsg>(include_bytes!(
        "../../../fixtures/native-async/v1/update-budget.json"
    ));
    roundtrip::<RuntimeMsg>(include_bytes!(
        "../../../fixtures/native-async/v1/register.json"
    ));
    roundtrip::<RuntimeMsg>(include_bytes!(
        "../../../fixtures/native-async/v1/deliver-error.json"
    ));
    roundtrip::<RuntimeMsg>(include_bytes!(
        "../../../fixtures/native-async/v1/fund.json"
    ));
    roundtrip::<TaskSudoMsg>(include_bytes!(
        "../../../fixtures/native-async/v1/poll-start.json"
    ));
    roundtrip::<TaskSudoMsg>(include_bytes!(
        "../../../fixtures/native-async/v1/poll-resume.json"
    ));
    roundtrip::<PollResponse>(include_bytes!(
        "../../../fixtures/native-async/v1/pending-height.json"
    ));
    roundtrip::<PollResponse>(include_bytes!(
        "../../../fixtures/native-async/v1/pending-operation.json"
    ));
    roundtrip::<PollResponse>(include_bytes!(
        "../../../fixtures/native-async/v1/complete.json"
    ));
    roundtrip::<PollResponse>(include_bytes!(
        "../../../fixtures/native-async/v1/failed.json"
    ));
}
