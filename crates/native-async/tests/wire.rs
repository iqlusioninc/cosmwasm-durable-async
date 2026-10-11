use native_async::{PollResponse, RuntimeMsg, TaskSudoMsg};
use serde::{de::DeserializeOwned, Serialize};

fn roundtrip<T: DeserializeOwned + Serialize>(bytes: &[u8]) {
    let typed: T = serde_json::from_slice(bytes).unwrap();
    let original: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), original);
}

#[test]
fn shared_go_rust_wire_fixtures() {
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
