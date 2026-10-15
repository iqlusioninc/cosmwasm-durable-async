#![allow(dead_code)] // Each integration target uses a different subset of these shared fixtures.
pub mod fixtures;
pub mod poll_host;
pub mod runtime_module;

use cosmwasm_std::{
    testing::{MockApi, MockStorage},
    *,
};
use cw_multi_test::{error::AnyResult, *};
use native_async::{runtime::BlockReport, *};
use runtime_module::{RunBlock, RuntimeModule};
use std::{cell::RefCell, rc::Rc};

pub type TestApp =
    App<BankKeeper, MockApi, MockStorage, RuntimeModule, WasmKeeper<RuntimeMsg, RuntimeQuery>>;
pub type FixtureWrites = (Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>);
pub struct Suite {
    pub app: TestApp,
    pub owner: Addr,
    pub contract: Addr,
    pub beneficiary: Addr,
    pub escrow: Addr,
    pub fees: Addr,
}

/// Both real example contracts, sharing the genuine transactional native host.
pub struct DcaSuite {
    pub app: TestApp,
    pub owner: Addr,
    pub sponsor: Addr,
    pub beneficiary: Addr,
    pub contract: Addr,
    pub exchange: Addr,
    pub fee_denom: String,
    pub fault_fixtures: bool,
}

pub fn dca_contract() -> Box<dyn Contract<RuntimeMsg, RuntimeQuery>> {
    Box::new(
        ContractWrapper::new(
            native_dca::execute,
            native_dca::instantiate,
            native_dca::query,
        )
        .with_reply(native_dca::reply)
        .with_sudo_empty(native_dca::sudo),
    )
}

impl DcaSuite {
    pub fn new() -> Self {
        Self::with_fee_denom("ugas")
    }
    pub fn with_fee_denom(fee_denom: &str) -> Self {
        Self::build(fee_denom, false)
    }
    pub fn with_faults() -> Self {
        Self::build("ugas", true)
    }
    fn build(fee_denom: &str, fault_fixtures: bool) -> Self {
        let mut app = app();
        app.update_block(|block| block.height = 1);
        let owner = app.api().addr_make("dca-owner");
        let sponsor = app.api().addr_make("dca-sponsor");
        let beneficiary = app.api().addr_make("dca-beneficiary");
        app.init_modules(|router, _, storage| {
            router.custom.runtime.keeper.config.denom = fee_denom.into();
            for account in [&owner, &sponsor] {
                router
                    .bank
                    .init_balance(
                        storage,
                        account,
                        vec![
                            coin(1_000_000, "ugas"),
                            coin(1_000_000, "uinput"),
                            coin(1_000_000, "uoutput"),
                        ],
                    )
                    .unwrap();
            }
        });
        let exchange_code = app.store_code(if fault_fixtures {
            fixtures::faulty_exchange()
        } else {
            Box::new(ContractWrapper::new_with_empty(
                mock_exchange::execute,
                mock_exchange::instantiate,
                mock_exchange::query,
            ))
        });
        let exchange = app
            .instantiate_contract(
                exchange_code,
                owner.clone(),
                &dca_example_protocol::ExchangeConfig {
                    input_denom: "uinput".into(),
                    output_denom: "uoutput".into(),
                    numerator: Uint128::new(3),
                    denominator: Uint128::new(2),
                },
                &[],
                "mock-exchange",
                None,
            )
            .unwrap();
        app.send_tokens(owner.clone(), exchange.clone(), &[coin(100_000, "uoutput")])
            .unwrap();
        let code = app.store_code(if fault_fixtures {
            fixtures::faulty_dca()
        } else {
            dca_contract()
        });
        let contract = app
            .instantiate_contract(
                code,
                owner.clone(),
                &native_dca::msg::InstantiateMsg {
                    exchange: exchange.to_string(),
                    fee_denom: fee_denom.into(),
                },
                &[],
                "native-dca",
                None,
            )
            .unwrap();
        if fault_fixtures {
            let relay_code = app.store_code(fixtures::relay());
            let relay = app
                .instantiate_contract(
                    relay_code,
                    owner.clone(),
                    &Empty {},
                    &[],
                    "fault-relay",
                    None,
                )
                .unwrap();
            app.contract_storage_mut(&exchange).set(
                fixtures::PROXY_KEY,
                &to_json_vec(&relay.to_string()).unwrap(),
            );
        }
        app.init_modules(|router, _, _| {
            router.custom.identities.insert(
                contract.to_string(),
                Binary::from(b"immutable-rust-native-dca-v1".as_slice()),
            );
        });
        Self {
            app,
            owner,
            sponsor,
            beneficiary,
            contract,
            exchange,
            fee_denom: fee_denom.into(),
            fault_fixtures,
        }
    }
    pub fn request(&self) -> native_dca::msg::CreateOrderRequest {
        native_dca::msg::CreateOrderRequest {
            input_per_slice: coin(10, "uinput"),
            slice_count: 3,
            minimum_output: Uint128::new(14),
            interval_blocks: 5,
            beneficiary: self.beneficiary.to_string(),
            gas: coin(1_000, &self.fee_denom),
            gas_per_poll: 100,
            max_gas_price: Decimal::one(),
            expires_at: 100,
            allocated_bytes: native_dca::contract::MIN_ALLOCATED_BYTES,
        }
    }
    pub fn funds(&self, request: &native_dca::msg::CreateOrderRequest) -> Vec<Coin> {
        let principal = request
            .input_per_slice
            .amount
            .checked_mul(Uint128::from(request.slice_count))
            .unwrap();
        if request.input_per_slice.denom == request.gas.denom {
            vec![Coin {
                denom: request.gas.denom.clone(),
                amount: principal.checked_add(request.gas.amount).unwrap(),
            }]
        } else {
            vec![
                Coin {
                    denom: request.input_per_slice.denom.clone(),
                    amount: principal,
                },
                request.gas.clone(),
            ]
        }
    }
    pub fn create(
        &mut self,
        owner: Addr,
        request: native_dca::msg::CreateOrderRequest,
    ) -> AnyResult<native_dca::msg::CreateOrderResponse> {
        let funds = self.funds(&request);
        let response = self.app.execute_contract(
            owner,
            self.contract.clone(),
            &native_dca::msg::ExecuteMsg::CreateOrder(request),
            &funds,
        )?;
        Ok(from_json(response.data.unwrap())?)
    }
    pub fn order(&self, order_id: u64) -> native_dca::msg::OrderView {
        self.app
            .wrap()
            .query_wasm_smart(
                &self.contract,
                &native_dca::msg::QueryMsg::Order { order_id },
            )
            .unwrap()
    }
    pub fn task(&self, order_id: u64) -> TaskResponse {
        self.app
            .wrap()
            .query_wasm_smart(
                &self.contract,
                &native_dca::msg::QueryMsg::Task { order_id },
            )
            .unwrap()
    }
    pub fn funding(&self, task_id: u64) -> TaskFunding {
        self.app
            .wrap()
            .query_wasm_smart(
                &self.contract,
                &native_dca::msg::QueryMsg::Async(AsyncQueryMsg::TaskFunding { task_id }),
            )
            .unwrap()
    }
    pub fn block(&mut self, height: u64) -> AnyResult<BlockReport> {
        self.app.update_block(|b| b.height = height);
        let block = self.app.block_info();
        let output = Rc::new(RefCell::new(None));
        self.app.init_modules(|router, api, storage| {
            router
                .custom
                .sudo(api, storage, router, &block, RunBlock(output.clone()))
        })?;
        let report = output
            .borrow_mut()
            .take()
            .expect("custom sudo returned report");
        Ok(report)
    }
    pub fn set_fault(&mut self, mode: fixtures::Fault) {
        for address in [&self.contract, &self.exchange] {
            self.app
                .contract_storage_mut(address)
                .set(fixtures::FAULT_KEY, &to_json_vec(&mode).unwrap());
        }
    }
    pub fn fill(&self, order_id: u64, slice_index: u32) -> Option<native_dca::state::FillRecord> {
        self.app
            .wrap()
            .query_wasm_smart(
                &self.contract,
                &native_dca::msg::QueryMsg::Fill {
                    order_id,
                    slice_index,
                },
            )
            .unwrap()
    }
    pub fn receipt(
        &self,
        order_id: u64,
        slice_index: u32,
    ) -> Option<dca_example_protocol::SwapReceipt> {
        self.app
            .wrap()
            .query_wasm_smart(
                &self.exchange,
                &dca_example_protocol::ExchangeQueryMsg::Receipt {
                    dca: self.contract.to_string(),
                    order_id,
                    slice_index,
                },
            )
            .unwrap()
    }
    pub fn conservation(&self, order_id: u64) {
        let order = self.order(order_id).order;
        assert_eq!(
            order.initial_principal,
            order.spent + order.remaining + order.reclaimed
        );
        assert!(order.pending.is_none());
        assert!(order.filled_count <= order.slice_count);
        for index in 0..order.slice_count {
            let fill = self.fill(order_id, index);
            let receipt = self.receipt(order_id, index);
            assert_eq!(fill.is_some(), index < order.filled_count);
            assert_eq!(receipt, fill.map(|fill| fill.receipt));
        }
    }
    pub fn reconstruct(&mut self) -> AnyResult<()> {
        let records: Vec<_> = self
            .app
            .storage()
            .range(None, None, Order::Ascending)
            .collect();
        let block = self.app.block_info();
        let mut rebuilt = app();
        assert_eq!(
            rebuilt.store_code(if self.fault_fixtures {
                fixtures::faulty_exchange()
            } else {
                Box::new(ContractWrapper::new_with_empty(
                    mock_exchange::execute,
                    mock_exchange::instantiate,
                    mock_exchange::query,
                ))
            }),
            1
        );
        assert_eq!(
            rebuilt.store_code(if self.fault_fixtures {
                fixtures::faulty_dca()
            } else {
                dca_contract()
            }),
            2
        );
        if self.fault_fixtures {
            assert_eq!(rebuilt.store_code(fixtures::relay()), 3);
        }
        for (key, value) in records {
            rebuilt.storage_mut().set(&key, &value);
        }
        rebuilt.set_block(block);
        assert_eq!(
            rebuilt
                .wrap()
                .query_wasm_contract_info(&self.contract)?
                .code_id,
            2
        );
        assert_eq!(
            rebuilt
                .wrap()
                .query_wasm_contract_info(&self.exchange)?
                .code_id,
            1
        );
        rebuilt.init_modules(|router, _, _| {
            router.custom.identities.insert(
                self.contract.to_string(),
                Binary::from(b"immutable-rust-native-dca-v1".as_slice()),
            );
        });
        self.app = rebuilt;
        Ok(())
    }
}
fn app() -> TestApp {
    let api = MockApi::default();
    BasicAppBuilder::<RuntimeMsg, RuntimeQuery>::new_custom()
        .with_custom(RuntimeModule::new(&api))
        .build(no_init)
}
impl Suite {
    pub fn new() -> Self {
        let mut app = app();
        app.update_block(|block| block.height = 1);
        let owner = app.api().addr_make("owner");
        let beneficiary = app.api().addr_make("beneficiary");
        let escrow = app.api().addr_make("escrow");
        let fees = app.api().addr_make("fees");
        app.init_modules(|router, _, storage| {
            router.bank.init_balance(
                storage,
                &owner,
                vec![coin(100_000, "ugas"), coin(100, "uinput")],
            )
        })
        .unwrap();
        let code_id = app.store_code(fixtures::contract());
        assert_eq!(code_id, 1);
        let contract = app
            .instantiate_contract(code_id, owner.clone(), &Empty {}, &[], "fixture", None)
            .unwrap();
        let mut suite = Self {
            app,
            owner,
            contract,
            beneficiary,
            escrow,
            fees,
        };
        suite.rebuild_identities().unwrap();
        suite
    }
    fn rebuild_identities(&mut self) -> AnyResult<()> {
        let info = self.app.wrap().query_wasm_contract_info(&self.contract)?;
        if info.code_id != 1 {
            cw_multi_test::error::bail!("unexpected fixture code id");
        }
        let identity = Binary::from(b"immutable-rust-adapter-fixture-v1".as_slice());
        self.app.init_modules(|router, _, _| {
            router.custom.identities.clear();
            router
                .custom
                .identities
                .insert(self.contract.to_string(), identity);
        });
        Ok(())
    }
    pub fn register(&mut self, fail: bool) {
        self.app
            .execute_contract(
                self.owner.clone(),
                self.contract.clone(),
                &fixtures::Execute::Register { fail },
                &[coin(1_000, "ugas"), coin(100, "uinput")],
            )
            .unwrap();
    }
    pub fn block(&mut self, height: u64) -> AnyResult<BlockReport> {
        self.app.update_block(|b| b.height = height);
        let block = self.app.block_info();
        let output = Rc::new(RefCell::new(None));
        self.app.init_modules(|router, api, storage| {
            router
                .custom
                .sudo(api, storage, router, &block, RunBlock(output.clone()))
        })?;
        let report = output
            .borrow_mut()
            .take()
            .expect("custom sudo returned report");
        Ok(report)
    }
    pub fn reconstruct(&mut self) -> AnyResult<()> {
        let records: Vec<_> = self
            .app
            .storage()
            .range(None, None, Order::Ascending)
            .collect();
        let block = self.app.block_info();
        let mut rebuilt = app();
        assert_eq!(rebuilt.store_code(fixtures::contract()), 1);
        for (key, value) in records {
            rebuilt.storage_mut().set(&key, &value);
        }
        rebuilt.set_block(block);
        self.app = rebuilt;
        self.rebuild_identities()
    }
    pub fn task(&self) -> TaskResponse {
        self.app
            .wrap()
            .query(&QueryRequest::Custom(RuntimeQuery::Task {
                contract: self.contract.to_string(),
                task_id: 1,
            }))
            .unwrap()
    }
    pub fn funding(&self) -> TaskFunding {
        self.app
            .wrap()
            .query(&QueryRequest::Custom(RuntimeQuery::Funding(
                SchedulerQuery::TaskFunding {
                    contract: self.contract.to_string(),
                    task_id: 1,
                },
            )))
            .unwrap()
    }
    pub fn business_balances(&self) -> Vec<Coin> {
        [&self.contract, &self.beneficiary]
            .into_iter()
            .map(|addr| self.app.wrap().query_balance(addr, "uinput").unwrap())
            .collect()
    }
    pub fn writes(&self) -> FixtureWrites {
        self.app
            .wrap()
            .query_wasm_smart(&self.contract, &Empty {})
            .unwrap()
    }
}
