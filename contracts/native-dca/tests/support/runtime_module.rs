use super::poll_host::RouterPollHost;
use cosmwasm_std::*;
use cw_multi_test::{error::AnyResult, *};
use native_async::{host::Keeper, runtime::*, *};
use serde::de::DeserializeOwned;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

pub struct RunBlock(pub Rc<RefCell<Option<BlockReport>>>);

pub struct RuntimeModule {
    pub runtime: Runtime,
    // Immutable Rust fixture identities, rebuilt from contract info on reconstruction.
    // These are not Wasm checksums and establish no migration guarantee.
    pub identities: BTreeMap<String, Binary>,
}

impl RuntimeModule {
    pub fn new(api: &testing::MockApi) -> Self {
        Self {
            identities: BTreeMap::new(),
            runtime: Runtime {
                keeper: Keeper {
                    config: FundingConfig {
                        denom: "ugas".into(),
                        gas_price: Decimal::one(),
                        min_base_price: Decimal::one(),
                        target_gas: 500,
                        price_grace_blocks: 3,
                        last_block_poll_gas_used: 0,
                        max_gas_per_poll: 1_000,
                        max_lifetime_blocks: 1_000,
                    },
                    escrow: api.addr_make("escrow"),
                },
                config: RuntimeConfig {
                    block_gas: 1_000,
                    price_grace_blocks: 3,
                    price_adjust_denominator: 8,
                    max_price_pauses: 100,
                    host_gas: 2,
                    maintenance_gas: 1_000,
                    maintenance_gas_per_item: 1,
                    max_visits: 100,
                    max_promotions: 100,
                    max_expirations: 100,
                    max_prunes: 100,
                    max_tasks: 100,
                    max_contract_tasks: 100,
                    max_allocated_bytes: 10_000_000,
                    max_task_bytes: 100_000,
                    max_messages: 16,
                    max_message_bytes: 20_000,
                    lease_price_per_byte_block: Uint128::zero(),
                    cleanup_fee: Uint128::one(),
                    retention_blocks: 10,
                    fee_collector: api.addr_make("fees"),
                },
            },
        }
    }
}

impl Module for RuntimeModule {
    type ExecT = RuntimeMsg;
    type QueryT = RuntimeQuery;
    type SudoT = RunBlock;

    fn execute<ExecC, QueryC>(
        &self,
        api: &dyn Api,
        storage: &mut dyn Storage,
        router: &dyn CosmosRouter<ExecC = ExecC, QueryC = QueryC>,
        block: &BlockInfo,
        sender: Addr,
        msg: RuntimeMsg,
    ) -> AnyResult<AppResponse>
    where
        ExecC: CustomMsg + DeserializeOwned + 'static,
        QueryC: CustomQuery + DeserializeOwned + 'static,
    {
        router.query(
            api,
            storage,
            block,
            QueryRequest::<QueryC>::Wasm(WasmQuery::ContractInfo {
                contract_addr: sender.to_string(),
            }),
        )?;
        let phase = storage.get(super::poll_host::PHASE_KEY).is_some();
        let result = self.runtime.execute(
            api,
            storage,
            block.height,
            &sender,
            msg,
            phase,
            &mut RouterPollHost {
                api,
                router,
                block,
                identities: &self.identities,
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
        msg: RuntimeQuery,
    ) -> AnyResult<Binary> {
        Ok(self.runtime.query(api, storage, block.height, msg)?)
    }

    fn sudo<ExecC, QueryC>(
        &self,
        api: &dyn Api,
        storage: &mut dyn Storage,
        router: &dyn CosmosRouter<ExecC = ExecC, QueryC = QueryC>,
        block: &BlockInfo,
        msg: RunBlock,
    ) -> AnyResult<AppResponse>
    where
        ExecC: CustomMsg + DeserializeOwned + 'static,
        QueryC: CustomQuery + DeserializeOwned + 'static,
    {
        let report = self.runtime.run_block(
            storage,
            block.height,
            &mut RouterPollHost {
                api,
                router,
                block,
                identities: &self.identities,
            },
        )?;
        // Output only: no scheduler, contract or bank state lives in this cell.
        *msg.0.borrow_mut() = Some(report);
        Ok(AppResponse::default())
    }
}
