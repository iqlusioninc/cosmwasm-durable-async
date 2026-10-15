use cosmwasm_std::*;
use cw_multi_test::{CosmosRouter, WasmSudo};
use native_async::{runtime::*, *};
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;

pub const PHASE_KEY: &[u8] = b"native-dca-test/scheduler-phase";
// Logical test schedule only: 2 runtime host units, 7 per sudo poll, and 5 per
// returned dispatch message (including all its nested calls). Positive base
// price floor = 1 ugas/unit. cw-multi-test supplies no SDK/VM gas measurement.
pub const POLL_GAS: u64 = 7;
pub const MESSAGE_GAS: u64 = 5;
pub struct RouterPollHost<'a, ExecC, QueryC> {
    pub api: &'a dyn Api,
    pub router: &'a dyn CosmosRouter<ExecC = ExecC, QueryC = QueryC>,
    pub block: &'a BlockInfo,
    pub identities: &'a BTreeMap<String, Binary>,
}
impl<ExecC, QueryC> PollHost for RouterPollHost<'_, ExecC, QueryC>
where
    ExecC: CustomMsg + DeserializeOwned + 'static,
    QueryC: CustomQuery + DeserializeOwned + 'static,
{
    fn code_checksum(&self, contract: &str) -> StdResult<Binary> {
        self.identities
            .get(contract)
            .cloned()
            .ok_or_else(|| StdError::generic_err("unknown immutable Rust fixture identity"))
    }
    fn poll(
        &mut self,
        storage: &mut dyn Storage,
        contract: &str,
        request: &PollRequest,
        gas_limit: u64,
    ) -> StdResult<Metered<PollResponse>> {
        // Broken fixture registration/infrastructure must fail the test, not pause a task.
        self.router
            .query(
                self.api,
                storage,
                self.block,
                QueryRequest::<QueryC>::Wasm(WasmQuery::ContractInfo {
                    contract_addr: contract.into(),
                }),
            )
            .map_err(|e| StdError::generic_err(e.to_string()))?;
        if POLL_GAS > gas_limit {
            return Ok(Metered {
                gas_used: POLL_GAS,
                result: Err(PauseReason::OutOfGas),
            });
        }
        let response = self.router.sudo(
            self.api,
            storage,
            self.block,
            WasmSudo::new(
                &Addr::unchecked(contract),
                &TaskSudoMsg::PollTask(request.clone()),
            )?
            .into(),
        );
        let result = match response {
            Err(_) => Err(PauseReason::Trap),
            Ok(response) => response
                .data
                .and_then(|data| from_json::<PollResponse>(data).ok())
                .ok_or(PauseReason::InvalidResponse),
        };
        Ok(Metered {
            gas_used: POLL_GAS,
            result,
        })
    }
    fn dispatch(
        &mut self,
        storage: &mut dyn Storage,
        contract: &str,
        messages: &[CosmosMsg],
        gas_limit: u64,
    ) -> StdResult<Metered<()>> {
        let previous = storage.get(PHASE_KEY);
        storage.set(PHASE_KEY, b"dispatch");
        let result = self.dispatch_messages(storage, contract, messages, gas_limit);
        // Restore even when conversion/infrastructure or a nested call fails.
        match previous {
            Some(value) => storage.set(PHASE_KEY, &value),
            None => storage.remove(PHASE_KEY),
        }
        result
    }
    fn transfer(
        &mut self,
        storage: &mut dyn Storage,
        from: &Addr,
        to: &Addr,
        coin: &Coin,
    ) -> StdResult<()> {
        self.router
            .execute(
                self.api,
                storage,
                self.block,
                from.clone(),
                CosmosMsg::Bank(BankMsg::Send {
                    to_address: to.to_string(),
                    amount: vec![coin.clone()],
                }),
            )
            .map(|_| ())
            .map_err(|e| StdError::generic_err(e.to_string()))
    }
}

impl<ExecC, QueryC> RouterPollHost<'_, ExecC, QueryC>
where
    ExecC: CustomMsg + DeserializeOwned + 'static,
    QueryC: CustomQuery + DeserializeOwned + 'static,
{
    fn dispatch_messages(
        &self,
        storage: &mut dyn Storage,
        contract: &str,
        messages: &[CosmosMsg],
        gas_limit: u64,
    ) -> StdResult<Metered<()>> {
        let mut gas_used: u64 = 0;
        for message in messages {
            let message = message
                .clone()
                .change_custom::<ExecC>()
                .ok_or_else(|| StdError::generic_err("unexpected Empty custom dispatch"))?;
            if let CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, .. }) = &message {
                self.router
                    .query(
                        self.api,
                        storage,
                        self.block,
                        QueryRequest::<QueryC>::Wasm(WasmQuery::ContractInfo {
                            contract_addr: contract_addr.clone(),
                        }),
                    )
                    .map_err(|e| StdError::generic_err(e.to_string()))?;
            }
            gas_used = gas_used.saturating_add(MESSAGE_GAS);
            if gas_used > gas_limit {
                return Ok(Metered {
                    gas_used,
                    result: Err(PauseReason::OutOfGas),
                });
            }
            if self
                .router
                .execute(
                    self.api,
                    storage,
                    self.block,
                    Addr::unchecked(contract),
                    message,
                )
                .is_err()
            {
                return Ok(Metered {
                    gas_used,
                    result: Err(PauseReason::MessageFailure),
                });
            }
        }
        Ok(Metered {
            gas_used,
            result: Ok(()),
        })
    }
}
