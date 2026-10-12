use cosmwasm_std::{
    testing::{MockApi, MockStorage},
    *,
};
use cw_multi_test::{
    error::{bail, AnyResult},
    *,
};
use native_async::{host::Keeper, *};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub struct SchedulerModule {
    pub keeper: Keeper,
}

impl Module for SchedulerModule {
    type ExecT = SchedulerMsg;
    type QueryT = SchedulerQuery;
    type SudoT = Empty;

    fn execute<ExecC, QueryC>(
        &self,
        api: &dyn Api,
        storage: &mut dyn Storage,
        router: &dyn CosmosRouter<ExecC = ExecC, QueryC = QueryC>,
        block: &BlockInfo,
        sender: Addr,
        msg: SchedulerMsg,
    ) -> AnyResult<AppResponse>
    where
        ExecC: CustomMsg + DeserializeOwned + 'static,
        QueryC: CustomQuery + DeserializeOwned + 'static,
    {
        // Custom messages can also be submitted by accounts in multi-test.
        // Reject them: native async messages require executing-contract provenance.
        router.query(
            api,
            storage,
            block,
            QueryRequest::<QueryC>::Wasm(WasmQuery::ContractInfo {
                contract_addr: sender.to_string(),
            }),
        )?;
        let result = self.keeper.execute(
            api,
            storage,
            block.height,
            &sender,
            msg,
            |storage, from, to, funds| {
                router
                    .execute(
                        api,
                        storage,
                        block,
                        from.clone(),
                        CosmosMsg::Bank(BankMsg::Send {
                            to_address: to.to_string(),
                            amount: vec![funds.clone()],
                        }),
                    )
                    .map(|_| ())
                    .map_err(|e| StdError::generic_err(e.to_string()))
            },
        )?;
        Ok(AppResponse {
            events: result.events,
            data: result.data,
            ..AppResponse::default()
        })
    }

    fn query(
        &self,
        api: &dyn Api,
        storage: &dyn Storage,
        _: &dyn Querier,
        block: &BlockInfo,
        msg: SchedulerQuery,
    ) -> AnyResult<Binary> {
        Ok(self.keeper.query(api, storage, block.height, msg)?)
    }

    fn sudo<ExecC, QueryC>(
        &self,
        _: &dyn Api,
        _: &mut dyn Storage,
        _: &dyn CosmosRouter<ExecC = ExecC, QueryC = QueryC>,
        _: &BlockInfo,
        _: Empty,
    ) -> AnyResult<AppResponse>
    where
        ExecC: CustomMsg + DeserializeOwned + 'static,
        QueryC: CustomQuery + DeserializeOwned + 'static,
    {
        bail!("unsupported sudo")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Execute {
    Register { gas_funds: Uint128 },
    Async(AsyncExecuteMsg),
    FundThenFail { task_id: TaskId },
    // Deliberately dishonest contract path: proves the host debits actual coins.
    ClaimFunds { task_id: TaskId, amount: Uint128 },
}

fn contract() -> Box<dyn Contract<SchedulerMsg, SchedulerQuery>> {
    fn instantiate(
        _: DepsMut<SchedulerQuery>,
        _: Env,
        _: MessageInfo,
        _: Empty,
    ) -> StdResult<Response<SchedulerMsg>> {
        Ok(Response::new())
    }
    fn execute(
        _: DepsMut<SchedulerQuery>,
        env: Env,
        info: MessageInfo,
        msg: Execute,
    ) -> StdResult<Response<SchedulerMsg>> {
        match msg {
            Execute::Register { gas_funds } => {
                let attached = attached_gas(&info, "ugas")?;
                if gas_funds.is_zero() || gas_funds > attached.amount {
                    return Err(StdError::generic_err("invalid split"));
                }
                Ok(
                    Response::new().add_message(CosmosMsg::Custom(SchedulerMsg::RegisterTask {
                        control: info.sender.to_string(),
                        refund: info.sender.to_string(),
                        gas_per_poll: 100,
                        max_gas_price: Decimal::one(),
                        expires_at: env.block.height + 10,
                        funds: Coin::new(gas_funds.u128(), "ugas"),
                    })),
                )
            }
            Execute::Async(msg) => execute_async(&info, msg, "ugas"),
            Execute::FundThenFail { task_id } => Ok(fund_task(&info, task_id, "ugas")?
                .add_message(BankMsg::Send {
                    to_address: info.sender.to_string(),
                    amount: coins(1_000_000, "ugas"),
                })),
            Execute::ClaimFunds { task_id, amount } => Ok(Response::new().add_message(
                CosmosMsg::Custom(SchedulerMsg::FundTask {
                    task_id,
                    contributor: info.sender.to_string(),
                    funds: Coin::new(amount.u128(), "ugas"),
                }),
            )),
        }
    }
    fn query(deps: Deps<SchedulerQuery>, env: Env, msg: AsyncQueryMsg) -> StdResult<Binary> {
        query_funding(deps, &env, msg)
    }
    Box::new(ContractWrapper::new(execute, instantiate, query))
}

pub type TestApp = App<
    BankKeeper,
    MockApi,
    MockStorage,
    SchedulerModule,
    WasmKeeper<SchedulerMsg, SchedulerQuery>,
>;
pub struct Suite {
    pub app: TestApp,
    pub owner: Addr,
    pub sponsor: Addr,
    pub contract: Addr,
    pub other: Addr,
    pub escrow: Addr,
}

impl Suite {
    pub fn new() -> Self {
        let api = MockApi::default();
        let owner = api.addr_make("owner");
        let sponsor = api.addr_make("sponsor");
        let escrow = api.addr_make("scheduler-escrow");
        let mut app = BasicAppBuilder::<SchedulerMsg, SchedulerQuery>::new_custom()
            .with_custom(SchedulerModule {
                keeper: Keeper {
                    config: FundingConfig {
                        denom: "ugas".into(),
                        gas_price: Decimal::percent(10),
                        max_gas_per_poll: 1_000,
                        max_lifetime_blocks: 100,
                    },
                    escrow: escrow.clone(),
                },
            })
            .build(|router, _, storage| {
                for address in [&owner, &sponsor] {
                    router
                        .bank
                        .init_balance(
                            storage,
                            address,
                            vec![coin(10_000, "ugas"), coin(100, "wrong")],
                        )
                        .unwrap();
                }
            });
        let code = app.store_code(contract());
        let contract = app
            .instantiate_contract(code, owner.clone(), &Empty {}, &[], "async", None)
            .unwrap();
        let other = app
            .instantiate_contract(code, owner.clone(), &Empty {}, &[], "other", None)
            .unwrap();
        Self {
            app,
            owner,
            sponsor,
            contract,
            other,
            escrow,
        }
    }
    pub fn register(&mut self, attached: u128, gas: u128) {
        self.app
            .execute_contract(
                self.owner.clone(),
                self.contract.clone(),
                &Execute::Register {
                    gas_funds: gas.into(),
                },
                &coins(attached, "ugas"),
            )
            .unwrap();
    }
    pub fn task(&self) -> TaskFunding {
        self.app
            .wrap()
            .query_wasm_smart(&self.contract, &AsyncQueryMsg::TaskFunding { task_id: 1 })
            .unwrap()
    }
    pub fn balance(&self, addr: &Addr) -> u128 {
        self.app
            .wrap()
            .query_balance(addr, "ugas")
            .unwrap()
            .amount
            .u128()
    }
    pub fn pause(&mut self, resume: ResumeState) {
        self.app.init_modules(|router, _, storage| {
            router
                .custom
                .keeper
                .pause(
                    storage,
                    &self.contract,
                    1,
                    PauseReason::InsufficientFunds,
                    resume,
                )
                .unwrap()
        });
    }
    pub fn execute(
        &mut self,
        sender: Addr,
        msg: AsyncExecuteMsg,
        funds: &[Coin],
    ) -> AnyResult<AppResponse> {
        self.app
            .execute_contract(sender, self.contract.clone(), &Execute::Async(msg), funds)
    }
}
