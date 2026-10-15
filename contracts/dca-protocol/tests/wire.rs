use cosmwasm_std::{coin, from_json, to_json_binary, Uint128};
use dca_example_protocol::{
    Correlation, DcaCallbackMsg, ExchangeConfig, ExchangeExecuteMsg, ExchangeQueryMsg, SwapReceipt,
    SwapRequest, MAX_RESULT_BYTES, MAX_SLICES,
};

const REQUEST: &str = r#"{"order_id":7,"slice_index":0,"correlation":{"workflow_id":41,"wait_sequence":2},"input":{"denom":"uin","amount":"10"},"output_denom":"uout","minimum_output":"14","beneficiary":"beneficiary"}"#;
const RECEIPT: &str = r#"{"order_id":7,"slice_index":0,"correlation":{"workflow_id":41,"wait_sequence":2},"input":{"denom":"uin","amount":"10"},"output":{"denom":"uout","amount":"15"},"beneficiary":"beneficiary"}"#;

fn receipt() -> SwapReceipt {
    SwapReceipt {
        order_id: 7,
        slice_index: 0,
        correlation: Correlation {
            workflow_id: 41,
            wait_sequence: 2,
        },
        input: coin(10, "uin"),
        output: coin(15, "uout"),
        beneficiary: "beneficiary".into(),
    }
}

#[test]
fn wire_roundtrip_preserves_distinct_ids() {
    let decoded: SwapRequest = from_json(REQUEST).unwrap();
    assert_eq!(decoded.order_id, 7);
    assert_eq!(decoded.correlation.workflow_id, 41);
    assert_eq!(decoded.slice_index, 0);
    assert_eq!(decoded.correlation.wait_sequence, 2);
    assert_eq!(decoded.input, coin(10, "uin"));
    assert_eq!(decoded.output_denom, "uout");
    assert_eq!(decoded.minimum_output, Uint128::new(14));
    assert_eq!(decoded.beneficiary, "beneficiary");
    assert_eq!(
        to_json_binary(&decoded).unwrap().as_slice(),
        REQUEST.as_bytes()
    );
    let msg = ExchangeExecuteMsg::Swap(decoded);
    let expected = format!(r#"{{"swap":{REQUEST}}}"#);
    assert_eq!(
        to_json_binary(&msg).unwrap().as_slice(),
        expected.as_bytes()
    );
    assert_eq!(from_json::<ExchangeExecuteMsg>(expected).unwrap(), msg);
    assert_eq!(MAX_SLICES, 128);
}

#[test]
fn callback_has_exact_snake_case_envelope() {
    let msg = DcaCallbackMsg::SwapResult { receipt: receipt() };
    let json = to_json_binary(&msg).unwrap();
    let expected = r#"{"swap_result":{"receipt":{"order_id":7,"slice_index":0,"correlation":{"workflow_id":41,"wait_sequence":2},"input":{"denom":"uin","amount":"10"},"output":{"denom":"uout","amount":"15"},"beneficiary":"beneficiary"}}}"#;
    assert!(std::str::from_utf8(json.as_slice())
        .unwrap()
        .starts_with("{\"swap_result\":{\"receipt\":"));
    assert_eq!(json.as_slice(), expected.as_bytes());
    assert_eq!(from_json::<DcaCallbackMsg>(expected).unwrap(), msg);
    assert_eq!(
        to_json_binary(&receipt()).unwrap().as_slice(),
        RECEIPT.as_bytes()
    );
}

#[test]
fn receipt_size_counts_json_escaping() {
    let mut value = receipt();
    value.beneficiary = "a\"\\\nλ".into();
    let expected = r#"{"order_id":7,"slice_index":0,"correlation":{"workflow_id":41,"wait_sequence":2},"input":{"denom":"uin","amount":"10"},"output":{"denom":"uout","amount":"15"},"beneficiary":"a\"\\\nλ"}"#;
    // 174 bytes of JSON framing/other fields plus 11 encoded beneficiary bytes:
    // a (1), escaped quote/backslash/newline (2 each), lambda (2), quotes (2).
    let encoded_len = 185;
    assert_eq!(
        to_json_binary(&value).unwrap().as_slice(),
        expected.as_bytes()
    );
    assert_eq!(to_json_binary(&value).unwrap().len(), encoded_len);
    assert_ne!(expected.chars().count(), encoded_len);
}

#[test]
fn boundary_receipts_have_exact_encoded_byte_lengths() {
    for (fixture, encoded_len) in [
        (include_str!("fixtures/receipt-2048.json"), 2048),
        (include_str!("fixtures/receipt-2049.json"), 2049),
    ] {
        let receipt: SwapReceipt = from_json(fixture.trim_end()).unwrap();
        assert_eq!(receipt.order_id, u64::MAX);
        assert_eq!(receipt.slice_index, u32::MAX);
        assert_eq!(receipt.correlation.workflow_id, u64::MAX);
        assert_eq!(receipt.correlation.wait_sequence, u64::MAX);
        assert_eq!(receipt.input.amount, Uint128::MAX);
        assert_eq!(receipt.output.amount, Uint128::MAX);
        let encoded = to_json_binary(&receipt).unwrap();
        assert_eq!(encoded.as_slice(), fixture.trim_end().as_bytes());
        assert_eq!(encoded.len(), encoded_len);
        assert_eq!(
            encoded.len() <= MAX_RESULT_BYTES as usize,
            encoded_len == 2048
        );
    }
}

#[test]
fn exchange_queries_and_config_have_exact_wire_shapes() {
    assert_eq!(
        to_json_binary(&ExchangeQueryMsg::Config {})
            .unwrap()
            .as_slice(),
        br#"{"config":{}}"#
    );
    let msg = ExchangeQueryMsg::Receipt {
        dca: "dca".into(),
        order_id: 7,
        slice_index: 0,
    };
    let expected = br#"{"receipt":{"dca":"dca","order_id":7,"slice_index":0}}"#;
    assert_eq!(to_json_binary(&msg).unwrap().as_slice(), expected);
    assert_eq!(from_json::<ExchangeQueryMsg>(expected).unwrap(), msg);
    let config = ExchangeConfig {
        input_denom: "uin".into(),
        output_denom: "uout".into(),
        numerator: Uint128::new(3),
        denominator: Uint128::new(2),
    };
    let expected =
        br#"{"input_denom":"uin","output_denom":"uout","numerator":"3","denominator":"2"}"#;
    assert_eq!(to_json_binary(&config).unwrap().as_slice(), expected);
    assert_eq!(from_json::<ExchangeConfig>(expected).unwrap(), config);
    assert_eq!(
        to_json_binary(&Option::<SwapReceipt>::None)
            .unwrap()
            .as_slice(),
        b"null"
    );
    assert_eq!(
        to_json_binary(&Some(receipt())).unwrap().as_slice(),
        RECEIPT.as_bytes()
    );
}
