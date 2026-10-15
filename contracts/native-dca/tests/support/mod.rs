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
