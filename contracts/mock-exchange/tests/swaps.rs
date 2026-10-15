use cosmwasm_std::{
    coin, from_json, to_json_binary, to_json_vec, Addr, Binary, Coin, Deps, DepsMut, Empty, Env,
    MessageInfo, Response, StdError, StdResult, Uint128, WasmMsg,
};
use cw_multi_test::{App, Contract, ContractWrapper, Executor};
use dca_example_protocol::{
    Correlation, ExchangeConfig, ExchangeExecuteMsg, ExchangeQueryMsg, SwapReceipt, SwapRequest,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ReceiverConfig {
    exchange: String,
    reject: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReceiverExecute {
    Swap { request: SwapRequest },
    SwapResult { receipt: SwapReceipt },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Observation {
    sender: String,
    receipt: SwapReceipt,
    beneficiary_balance: Coin,
}

fn receiver_instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    config: ReceiverConfig,
) -> StdResult<Response> {
    deps.storage.set(b"config", &to_json_vec(&config)?);
    Ok(Response::new())
}

fn receiver_execute(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: ReceiverExecute,
) -> StdResult<Response> {
    let config: ReceiverConfig = from_json(deps.storage.get(b"config").unwrap())?;
    match msg {
        ReceiverExecute::Swap { request } => Ok(Response::new().add_message(WasmMsg::Execute {
            contract_addr: config.exchange,
            msg: to_json_binary(&ExchangeExecuteMsg::Swap(request))?,
            funds: info.funds,
        })),
        ReceiverExecute::SwapResult { receipt } => {
            if info.sender.as_str() != config.exchange || !info.funds.is_empty() {
                return Err(StdError::generic_err("unauthenticated callback"));
            }
            if config.reject {
                return Err(StdError::generic_err("callback rejected"));
            }
            let beneficiary_balance = deps
                .querier
                .query_balance(&receipt.beneficiary, &receipt.output.denom)?;
            deps.storage.set(
                b"observation",
                &to_json_vec(&Observation {
                    sender: info.sender.to_string(),
                    receipt,
                    beneficiary_balance,
                })?,
            );
            Ok(Response::new())
        }
    }
}

fn receiver_query(deps: Deps, _env: Env, _msg: Empty) -> StdResult<Binary> {
    to_json_binary(
        &deps
            .storage
            .get(b"observation")
            .map(from_json::<Observation>)
            .transpose()?,
    )
}

fn exchange_contract() -> Box<dyn Contract<Empty>> {
    Box::new(ContractWrapper::new(
        mock_exchange::execute,
        mock_exchange::instantiate,
        mock_exchange::query,
    ))
}

fn config() -> ExchangeConfig {
    ExchangeConfig {
        input_denom: "uin".into(),
        output_denom: "uout".into(),
        numerator: Uint128::new(3),
        denominator: Uint128::new(2),
    }
}

struct Suite {
    app: App,
    owner: Addr,
    beneficiary: Addr,
    exchange: Addr,
    receiver: Addr,
    receiver_code: u64,
    exchange_code: u64,
}

impl Suite {
    fn new(liquidity: u128, reject: bool, terms: ExchangeConfig) -> Self {
        let mut app = App::default();
        let owner = app.api().addr_make("owner");
        let beneficiary = app.api().addr_make("beneficiary");
        app.init_modules(|router, _, storage| {
            router
                .bank
                .init_balance(
                    storage,
                    &owner,
                    vec![
                        coin(1000, "uin"),
                        coin(liquidity.max(1), "uout"),
                        coin(100, "extra"),
                    ],
                )
                .unwrap();
        });
        let exchange_code = app.store_code(exchange_contract());
        let exchange = app
            .instantiate_contract(exchange_code, owner.clone(), &terms, &[], "exchange", None)
            .unwrap();
        if liquidity > 0 {
            app.send_tokens(owner.clone(), exchange.clone(), &[coin(liquidity, "uout")])
                .unwrap();
        }
        let receiver_code = app.store_code(Box::new(ContractWrapper::new(
            receiver_execute,
            receiver_instantiate,
            receiver_query,
        )));
        let receiver = app
            .instantiate_contract(
                receiver_code,
                owner.clone(),
                &ReceiverConfig {
                    exchange: exchange.to_string(),
                    reject,
                },
                &[],
                "receiver",
                None,
            )
            .unwrap();
        Self {
            app,
            owner,
            beneficiary,
            exchange,
            receiver,
            receiver_code,
            exchange_code,
        }
    }

    fn request(&self) -> SwapRequest {
        SwapRequest {
            order_id: 7,
            slice_index: 0,
            correlation: Correlation {
                workflow_id: 41,
                wait_sequence: 2,
            },
            input: coin(10, "uin"),
            output_denom: "uout".into(),
            minimum_output: Uint128::new(14),
            beneficiary: self.beneficiary.to_string(),
        }
    }

    fn receipt(&self, requester: &Addr, request: &SwapRequest) -> Option<SwapReceipt> {
        self.app
            .wrap()
            .query_wasm_smart(
                &self.exchange,
                &ExchangeQueryMsg::Receipt {
                    dca: requester.to_string(),
                    order_id: request.order_id,
                    slice_index: request.slice_index,
                },
            )
            .unwrap()
    }

    fn balances(&self) -> Vec<Vec<Coin>> {
        [
            &self.owner,
            &self.beneficiary,
            &self.exchange,
            &self.receiver,
        ]
        .into_iter()
        .map(|a| {
            ["uin", "uout", "extra"]
                .into_iter()
                .map(|denom| self.app.wrap().query_balance(a, denom).unwrap())
                .collect()
        })
        .collect()
    }

    fn reject_swap(&mut self, request: SwapRequest, funds: &[Coin], error: &str) {
        let before = self.balances();
        let receipt_before = self.receipt(&self.receiver, &request);
        let err = self
            .app
            .execute_contract(
                self.owner.clone(),
                self.receiver.clone(),
                &ReceiverExecute::Swap {
                    request: request.clone(),
                },
                funds,
            )
            .unwrap_err();
        assert!(
            format!("{err:#}").contains(error),
            "unexpected error: {err:#}"
        );
        assert_eq!(self.balances(), before);
        assert_eq!(self.receipt(&self.receiver, &request), receipt_before);
        let observation: Option<Observation> = self
            .app
            .wrap()
            .query_wasm_smart(&self.receiver, &Empty {})
            .unwrap();
        if receipt_before.is_none() {
            assert!(observation.is_none());
        }
    }
}

#[test]
fn sends_output_before_authenticated_callback_to_actual_requester() {
    let mut suite = Suite::new(100, false, config());
    let request = suite.request();
    suite
        .app
        .execute_contract(
            suite.owner.clone(),
            suite.receiver.clone(),
            &ReceiverExecute::Swap {
                request: request.clone(),
            },
            &[coin(10, "uin")],
        )
        .unwrap();
    assert_eq!(
        suite
            .app
            .wrap()
            .query_balance(&suite.beneficiary, "uout")
            .unwrap(),
        coin(15, "uout")
    );
    assert_eq!(
        suite
            .app
            .wrap()
            .query_balance(&suite.exchange, "uin")
            .unwrap(),
        coin(10, "uin")
    );
    assert_eq!(
        suite
            .app
            .wrap()
            .query_balance(&suite.exchange, "uout")
            .unwrap(),
        coin(85, "uout")
    );
    let observation: Option<Observation> = suite
        .app
        .wrap()
        .query_wasm_smart(&suite.receiver, &Empty {})
        .unwrap();
    let observation = observation.unwrap();
    assert_eq!(observation.sender, suite.exchange.as_str());
    assert_eq!(observation.beneficiary_balance, coin(15, "uout"));
    let receipt = suite.receipt(&suite.receiver, &request).unwrap();
    assert_eq!(
        receipt,
        SwapReceipt {
            order_id: 7,
            slice_index: 0,
            correlation: request.correlation,
            input: coin(10, "uin"),
            output: coin(15, "uout"),
            beneficiary: suite.beneficiary.to_string(),
        }
    );
    assert_eq!(observation.receipt, receipt);
    assert_eq!(suite.receipt(&suite.owner, &request), None);
    let terms: ExchangeConfig = suite
        .app
        .wrap()
        .query_wasm_smart(&suite.exchange, &ExchangeQueryMsg::Config {})
        .unwrap();
    assert_eq!(terms, config());
}

#[test]
fn rejected_callback_rolls_back_bank_transfers_and_receipt() {
    let mut suite = Suite::new(100, true, config());
    suite.reject_swap(suite.request(), &[coin(10, "uin")], "callback rejected");
}

#[test]
fn quote_floors_without_narrow_product_overflow() {
    let mut terms = config();
    terms.numerator = Uint128::MAX;
    terms.denominator = Uint128::MAX;
    let mut suite = Suite::new(100, false, terms);
    let mut request = suite.request();
    request.minimum_output = Uint128::new(10);
    suite
        .app
        .execute_contract(
            suite.owner.clone(),
            suite.receiver.clone(),
            &ReceiverExecute::Swap {
                request: request.clone(),
            },
            &[coin(10, "uin")],
        )
        .unwrap();
    assert_eq!(
        suite.receipt(&suite.receiver, &request).unwrap().output,
        coin(10, "uout")
    );
    let mut suite = Suite::new(100, false, config());
    let mut request = suite.request();
    request.input = coin(11, "uin");
    suite
        .app
        .execute_contract(
            suite.owner.clone(),
            suite.receiver.clone(),
            &ReceiverExecute::Swap {
                request: request.clone(),
            },
            &[coin(11, "uin")],
        )
        .unwrap();
    assert_eq!(
        suite.receipt(&suite.receiver, &request).unwrap().output,
        coin(16, "uout")
    );
}

#[test]
fn zero_rounding_and_unrepresentable_quotes_preserve_funds() {
    let mut terms = config();
    terms.numerator = Uint128::one();
    terms.denominator = Uint128::new(3);
    let mut suite = Suite::new(100, false, terms);
    let mut request = suite.request();
    request.input = coin(1, "uin");
    request.minimum_output = Uint128::one();
    suite.reject_swap(request, &[coin(1, "uin")], "zero output");
    let mut terms = config();
    terms.numerator = Uint128::MAX;
    terms.denominator = Uint128::one();
    let mut suite = Suite::new(100, false, terms);
    suite.reject_swap(
        suite.request(),
        &[coin(10, "uin")],
        "unrepresentable output",
    );
}

#[test]
fn rejects_minimum_output_and_insufficient_liquidity_atomically() {
    let mut suite = Suite::new(100, false, config());
    let mut request = suite.request();
    request.minimum_output = Uint128::new(16);
    suite.reject_swap(request, &[coin(10, "uin")], "minimum output");
    let mut suite = Suite::new(14, false, config());
    suite.reject_swap(
        suite.request(),
        &[coin(10, "uin")],
        "insufficient liquidity",
    );
}

#[test]
fn rejects_invalid_terms_and_exact_funds_violations() {
    let mut suite = Suite::new(100, false, config());
    for (input, output, minimum, beneficiary, funds, error) in [
        (
            coin(0, "uin"),
            "uout",
            1,
            suite.beneficiary.to_string(),
            vec![],
            "positive input",
        ),
        (
            coin(10, "extra"),
            "uout",
            14,
            suite.beneficiary.to_string(),
            vec![coin(10, "extra")],
            "input denomination",
        ),
        (
            coin(10, "uin"),
            "extra",
            14,
            suite.beneficiary.to_string(),
            vec![coin(10, "uin")],
            "output denomination",
        ),
        (
            coin(10, "uin"),
            "uout",
            0,
            suite.beneficiary.to_string(),
            vec![coin(10, "uin")],
            "positive minimum",
        ),
        (
            coin(10, "uin"),
            "uout",
            14,
            "invalid".into(),
            vec![coin(10, "uin")],
            "bech32",
        ),
        (
            coin(10, "uin"),
            "uout",
            14,
            suite.beneficiary.to_string(),
            vec![],
            "exact input",
        ),
        (
            coin(10, "uin"),
            "uout",
            14,
            suite.beneficiary.to_string(),
            vec![coin(9, "uin")],
            "exact input",
        ),
        (
            coin(10, "uin"),
            "uout",
            14,
            suite.beneficiary.to_string(),
            vec![coin(11, "uin")],
            "exact input",
        ),
        (
            coin(10, "uin"),
            "uout",
            14,
            suite.beneficiary.to_string(),
            vec![coin(10, "uin"), coin(1, "extra")],
            "exact input",
        ),
        (
            coin(10, "uin"),
            "uout",
            14,
            suite.beneficiary.to_string(),
            vec![coin(5, "uin"), coin(5, "uin")],
            "exact input",
        ),
        (
            coin(10, "uin"),
            "uout",
            14,
            suite.beneficiary.to_string(),
            vec![coin(10, "uin"), coin(0, "extra")],
            "exact input",
        ),
    ] {
        let mut request = suite.request();
        request.input = input;
        request.output_denom = output.into();
        request.minimum_output = Uint128::new(minimum);
        request.beneficiary = beneficiary;
        suite.reject_swap(request, &funds, error);
    }
}

#[test]
fn duplicate_key_ignores_changed_correlation_and_scopes_actual_requester() {
    let mut suite = Suite::new(100, false, config());
    let request = suite.request();
    suite
        .app
        .execute_contract(
            suite.owner.clone(),
            suite.receiver.clone(),
            &ReceiverExecute::Swap {
                request: request.clone(),
            },
            &[coin(10, "uin")],
        )
        .unwrap();
    let mut duplicate = request.clone();
    duplicate.correlation.wait_sequence = 99;
    suite.reject_swap(duplicate, &[coin(10, "uin")], "duplicate swap");
    let second = suite
        .app
        .instantiate_contract(
            suite.receiver_code,
            suite.owner.clone(),
            &ReceiverConfig {
                exchange: suite.exchange.to_string(),
                reject: false,
            },
            &[],
            "second",
            None,
        )
        .unwrap();
    suite
        .app
        .execute_contract(
            suite.owner.clone(),
            second.clone(),
            &ReceiverExecute::Swap {
                request: request.clone(),
            },
            &[coin(10, "uin")],
        )
        .unwrap();
    assert!(suite.receipt(&second, &request).is_some());
    assert!(suite.receipt(&suite.receiver, &request).is_some());
    assert_eq!(
        suite
            .app
            .wrap()
            .query_balance(&suite.beneficiary, "uout")
            .unwrap(),
        coin(30, "uout")
    );
}

#[test]
fn instantiate_rejects_invalid_denoms_ratios_and_attached_funds() {
    let mut suite = Suite::new(100, false, config());
    for denom in ["", "ab", "1abc", "u in", "u☃", "a@b", &"a".repeat(129)] {
        for input in [true, false] {
            let mut terms = config();
            if input {
                terms.input_denom = denom.into();
            } else {
                terms.output_denom = denom.into();
            }
            assert!(
                suite
                    .app
                    .instantiate_contract(
                        suite.exchange_code,
                        suite.owner.clone(),
                        &terms,
                        &[],
                        "invalid",
                        None
                    )
                    .is_err(),
                "accepted denom {denom:?}"
            );
        }
    }
    for terms in [
        ExchangeConfig {
            output_denom: "uin".into(),
            ..config()
        },
        ExchangeConfig {
            numerator: Uint128::zero(),
            ..config()
        },
        ExchangeConfig {
            denominator: Uint128::zero(),
            ..config()
        },
    ] {
        assert!(suite
            .app
            .instantiate_contract(
                suite.exchange_code,
                suite.owner.clone(),
                &terms,
                &[],
                "invalid",
                None
            )
            .is_err());
    }
    let before = suite.balances();
    assert!(suite
        .app
        .instantiate_contract(
            suite.exchange_code,
            suite.owner.clone(),
            &config(),
            &[coin(1, "uin")],
            "funded",
            None
        )
        .is_err());
    assert_eq!(suite.balances(), before);
    for denom in ["abc", "a/BC:0-9_.", &"a".repeat(128)] {
        let terms = ExchangeConfig {
            input_denom: denom.into(),
            ..config()
        };
        suite
            .app
            .instantiate_contract(
                suite.exchange_code,
                suite.owner.clone(),
                &terms,
                &[],
                "valid",
                None,
            )
            .unwrap();
    }
}

#[test]
fn direct_account_request_rolls_back_when_callback_cannot_execute() {
    let mut suite = Suite::new(100, false, config());
    let request = suite.request();
    let before = suite.balances();
    assert!(suite
        .app
        .execute_contract(
            suite.owner.clone(),
            suite.exchange.clone(),
            &ExchangeExecuteMsg::Swap(request.clone()),
            &[coin(10, "uin")]
        )
        .is_err());
    assert_eq!(suite.balances(), before);
    assert_eq!(suite.receipt(&suite.owner, &request), None);
}
