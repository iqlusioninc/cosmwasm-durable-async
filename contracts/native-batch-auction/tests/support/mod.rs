#![allow(dead_code)]
pub mod instrumented;
pub mod poll_host;
pub mod runtime_module;

use cosmwasm_std::{
    testing::{MockApi, MockStorage},
    *,
};
use cw_multi_test::{error::AnyResult, *};
use native_async::{runtime::BlockReport, *};
use native_batch_auction::{msg::*, state::*, AuctionView};
use runtime_module::{RunBlock, RuntimeModule};
use serde::de::DeserializeOwned;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

const REJECT_KEY: &[u8] = b"auction-test/reject-bank";
pub const BAD_REPLY_KEY: &[u8] = b"auction-test/bad-reply";
pub struct RejectingBank(pub BankKeeper);
impl Bank for RejectingBank {}
impl Module for RejectingBank {
    type ExecT = BankMsg;
    type QueryT = BankQuery;
    type SudoT = BankSudo;
    fn execute<E, Q>(
        &self,
        api: &dyn Api,
        storage: &mut dyn Storage,
        router: &dyn CosmosRouter<ExecC = E, QueryC = Q>,
        block: &BlockInfo,
        sender: Addr,
        msg: BankMsg,
    ) -> AnyResult<AppResponse>
    where
        E: CustomMsg + DeserializeOwned + 'static,
        Q: CustomQuery + DeserializeOwned + 'static,
    {
        if let (Some(raw), BankMsg::Send { to_address, amount }) = (storage.get(REJECT_KEY), &msg) {
            let (recipient, denom): (String, String) = from_json(raw)?;
            if to_address == &recipient && amount.iter().any(|c| c.denom == denom) {
                cw_multi_test::error::bail!("injected bank rejection");
            }
        }
        self.0.execute(api, storage, router, block, sender, msg)
    }
    fn query(
        &self,
        api: &dyn Api,
        storage: &dyn Storage,
        querier: &dyn Querier,
        block: &BlockInfo,
        msg: BankQuery,
    ) -> AnyResult<Binary> {
        self.0.query(api, storage, querier, block, msg)
    }
    fn sudo<E, Q>(
        &self,
        api: &dyn Api,
        storage: &mut dyn Storage,
        router: &dyn CosmosRouter<ExecC = E, QueryC = Q>,
        block: &BlockInfo,
        msg: BankSudo,
    ) -> AnyResult<AppResponse>
    where
        E: CustomMsg + DeserializeOwned + 'static,
        Q: CustomQuery + DeserializeOwned + 'static,
    {
        self.0.sudo(api, storage, router, block, msg)
    }
}
pub type TestApp =
    App<RejectingBank, MockApi, MockStorage, RuntimeModule, WasmKeeper<RuntimeMsg, RuntimeQuery>>;
pub fn auction_contract() -> Box<dyn Contract<RuntimeMsg, RuntimeQuery>> {
    // The reply fault changes only test input; every normal path calls the real handler.
    Box::new(
        ContractWrapper::new(
            instrumented::execute,
            native_batch_auction::instantiate,
            native_batch_auction::query,
        )
        .with_reply(|deps: DepsMut<RuntimeQuery>, env, mut reply: Reply| {
            if deps.storage.get(BAD_REPLY_KEY).is_some() {
                reply.payload = Binary::from(br#"{"auction_id":2}"#.as_slice());
            }
            native_batch_auction::reply(deps, env, reply)
        })
        .with_sudo_empty(instrumented::sudo),
    )
}
fn app() -> TestApp {
    let api = MockApi::default();
    BasicAppBuilder::<RuntimeMsg, RuntimeQuery>::new_custom()
        .with_custom(RuntimeModule::new(&api))
        .with_bank(RejectingBank(BankKeeper::new()))
        .build(no_init)
}
pub struct AuctionSuite {
    pub app: TestApp,
    pub seller: Addr,
    pub sponsor: Addr,
    pub beneficiary: Addr,
    pub contract: Addr,
    pub fee_denom: String,
}
impl AuctionSuite {
    pub fn new() -> Self {
        Self::with_fee_denom("ugas")
    }
    pub fn with_fee_denom(denom: &str) -> Self {
        let mut app = app();
        app.update_block(|b| b.height = 1);
        let seller = app.api().addr_make("auction-seller");
        let sponsor = app.api().addr_make("auction-sponsor");
        let beneficiary = app.api().addr_make("auction-beneficiary");
        app.init_modules(|r, _, s| {
            r.custom.runtime.keeper.config.denom = denom.into();
            for a in [&seller, &sponsor] {
                r.bank
                    .0
                    .init_balance(
                        s,
                        a,
                        vec![
                            coin(1_000_000, "ugas"),
                            coin(1_000_000, "usale"),
                            coin(1_000_000, "uquote"),
                        ],
                    )
                    .unwrap();
            }
        });
        let id = app.store_code(auction_contract());
        let contract = app
            .instantiate_contract(
                id,
                seller.clone(),
                &InstantiateMsg {
                    sale_denom: "usale".into(),
                    quote_denom: "uquote".into(),
                    fee_denom: denom.into(),
                },
                &[],
                "auction",
                None,
            )
            .unwrap();
        app.init_modules(|r, _, _| {
            r.custom.identities.insert(
                contract.to_string(),
                Binary::from(b"immutable-rust-native-auction-v1".as_slice()),
            );
        });
        Self {
            app,
            seller,
            sponsor,
            beneficiary,
            contract,
            fee_denom: denom.into(),
        }
    }
    pub fn request(&self) -> CreateAuctionRequest {
        CreateAuctionRequest {
            terms: AuctionTerms {
                supply_lots: Uint128::new(5),
                lot_size: Uint128::new(10),
                reserve_price: Uint128::new(2),
                tick_size: Uint128::one(),
                tick_count: 3,
                max_bids: 5,
                batch_size: 1,
                commit_end: 10,
                reveal_end: 20,
                expires_at: 60,
            },
            gas: coin(1_000, &self.fee_denom),
            gas_per_poll: 100,
            max_gas_price: Decimal::one(),
            allocated_bytes: native_batch_auction::MIN_ALLOCATED_BYTES,
        }
    }
    pub fn funds(&self, r: &CreateAuctionRequest) -> Vec<Coin> {
        let mut funds =
            BTreeMap::from([("usale".to_string(), r.terms.supply_lots * r.terms.lot_size)]);
        *funds.entry(r.gas.denom.clone()).or_default() += r.gas.amount;
        funds
            .into_iter()
            .map(|(denom, amount)| Coin { denom, amount })
            .collect()
    }
    pub fn create(&mut self) -> AnyResult<CreateAuctionResponse> {
        let r = self.request();
        let f = self.funds(&r);
        let response = self.app.execute_contract(
            self.seller.clone(),
            self.contract.clone(),
            &ExecuteMsg::CreateAuction(r),
            &f,
        )?;
        Ok(from_json(response.data.unwrap())?)
    }
    pub fn execute(
        &mut self,
        actor: Addr,
        msg: &ExecuteMsg,
        funds: &[Coin],
    ) -> AnyResult<AppResponse> {
        self.app
            .execute_contract(actor, self.contract.clone(), msg, funds)
    }
    pub fn height(&mut self, height: u64) {
        self.app.update_block(|b| b.height = height);
    }
    pub fn auction(&self) -> AuctionView {
        self.app
            .wrap()
            .query_wasm_smart(&self.contract, &QueryMsg::Auction {})
            .unwrap()
    }
    pub fn bid(&self, bidder: &Addr) -> Option<BidRecord> {
        self.app
            .wrap()
            .query_wasm_smart(
                &self.contract,
                &QueryMsg::Bid {
                    bidder: bidder.to_string(),
                },
            )
            .unwrap()
    }
    pub fn balance(&self, account: &Addr, denom: &str) -> Uint128 {
        self.app
            .wrap()
            .query_balance(account, denom)
            .unwrap()
            .amount
    }
    pub fn funding(&self) -> TaskFunding {
        self.app
            .wrap()
            .query_wasm_smart(
                &self.contract,
                &QueryMsg::Async(AsyncQueryMsg::TaskFunding {
                    task_id: self.auction().auction.task_id.unwrap(),
                }),
            )
            .unwrap()
    }
    pub fn bidder(&mut self, label: &str) -> Addr {
        let a = self.app.api().addr_make(label);
        self.app
            .sudo(SudoMsg::Bank(BankSudo::Mint {
                to_address: a.to_string(),
                amount: vec![coin(1_000_000, "uquote")],
            }))
            .unwrap();
        a
    }
    pub fn digest(&self, bidder: &Addr, quantity: u128, tick: u16, collateral: u128) -> Binary {
        native_batch_auction::commitment::commitment_digest(
            &native_batch_auction::commitment::CommitmentInput {
                chain_id: &self.app.block_info().chain_id,
                contract: &self.contract,
                bidder,
                beneficiary: &self.beneficiary,
                auction_id: 1,
                collateral: Uint128::new(collateral),
                quantity_lots: Uint128::new(quantity),
                price_tick: tick,
                salt: &[7; 32],
            },
        )
        .unwrap()
        .into()
    }
    pub fn commit(
        &mut self,
        bidder: &Addr,
        quantity: u128,
        tick: u16,
        collateral: u128,
    ) -> AnyResult<AppResponse> {
        self.execute(
            bidder.clone(),
            &ExecuteMsg::CommitBid {
                digest: self.digest(bidder, quantity, tick, collateral),
                collateral: Uint128::new(collateral),
            },
            &[coin(collateral, "uquote")],
        )
    }
    pub fn reveal(&mut self, bidder: &Addr, quantity: u128, tick: u16) -> AnyResult<AppResponse> {
        self.execute(
            bidder.clone(),
            &ExecuteMsg::RevealBid(RevealBidRequest {
                quantity_lots: Uint128::new(quantity),
                price_tick: tick,
                beneficiary: self.beneficiary.to_string(),
                salt: Binary::from(vec![7; 32]),
            }),
            &[],
        )
    }
    pub fn reject_bank(&mut self, recipient: &Addr, denom: &str) {
        self.app.storage_mut().set(
            REJECT_KEY,
            &to_json_vec(&(recipient.to_string(), denom.to_string())).unwrap(),
        );
    }
    pub fn allow_bank(&mut self) {
        self.app.storage_mut().remove(REJECT_KEY);
    }
    pub fn block(&mut self, height: u64) -> AnyResult<BlockReport> {
        self.height(height);
        let block = self.app.block_info();
        let output = Rc::new(RefCell::new(None));
        self.app
            .init_modules(|r, a, s| r.custom.sudo(a, s, r, &block, RunBlock(output.clone())))?;
        let report = output.borrow_mut().take().unwrap();
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
        assert_eq!(rebuilt.store_code(auction_contract()), 1);
        for (k, v) in records {
            rebuilt.storage_mut().set(&k, &v);
        }
        rebuilt.set_block(block);
        assert_eq!(
            rebuilt
                .wrap()
                .query_wasm_contract_info(&self.contract)?
                .code_id,
            1
        );
        let config = load_config(&*rebuilt.contract_storage(&self.contract))?;
        rebuilt.init_modules(|r, _, _| {
            r.custom.runtime.keeper.config.denom = self.fee_denom.clone();
            r.custom.identities.insert(
                self.contract.to_string(),
                Binary::from(b"immutable-rust-native-auction-v1".as_slice()),
            );
        });
        assert_eq!(config.seller, self.seller);
        self.app = rebuilt;
        Ok(())
    }
    pub fn settle(&mut self) {
        let auction = self.auction().auction;
        for height in auction.terms.reveal_end..auction.terms.expires_at {
            self.block(height).unwrap();
            if self.auction().auction.state == BusinessState::Finalized {
                return;
            }
        }
        panic!("auction did not finalize before expiry");
    }
}
