//! Reference host accounting, not a deployed keeper. The chain adapter must
//! supply authenticated contract provenance, transactional storage, and a bank
//! transfer implementation. Never expose these methods as contract entry points.
use crate::*;
use cosmwasm_std::{
    from_json, to_json_binary, Addr, Api, Binary, Coin, Decimal, Event, StdError, StdResult,
    Storage, Uint128,
};

pub struct Keeper {
    pub config: FundingConfig,
    pub escrow: Addr,
}

pub struct HostResponse {
    pub events: Vec<Event>,
    pub data: Option<Binary>,
}

impl Keeper {
    /// The adapter MUST authenticate `contract` as the executing contract and
    /// run this call and ALL downstream effects in one transaction cache.
    /// `transfer` must debit `from` and credit `to`, or return an error. A
    /// no-op callback would mint unbacked credit and is never a valid adapter.
    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &self,
        api: &dyn Api,
        storage: &mut dyn Storage,
        height: u64,
        contract: &Addr,
        msg: SchedulerMsg,
        transfer: impl FnMut(&mut dyn Storage, &Addr, &Addr, &Coin) -> StdResult<()>,
    ) -> StdResult<HostResponse> {
        self.validate_config(false)?;
        self.execute_validated(api, storage, height, contract, msg, transfer)
    }

    /// Runtime pricing permits zero; the legacy public keeper requires a
    /// positive fixed price. All other funding validation remains shared.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_runtime(
        &self,
        api: &dyn Api,
        storage: &mut dyn Storage,
        height: u64,
        contract: &Addr,
        msg: SchedulerMsg,
        transfer: impl FnMut(&mut dyn Storage, &Addr, &Addr, &Coin) -> StdResult<()>,
    ) -> StdResult<HostResponse> {
        self.validate_config(true)?;
        self.execute_validated(api, storage, height, contract, msg, transfer)
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_validated(
        &self,
        api: &dyn Api,
        storage: &mut dyn Storage,
        height: u64,
        contract: &Addr,
        msg: SchedulerMsg,
        mut transfer: impl FnMut(&mut dyn Storage, &Addr, &Addr, &Coin) -> StdResult<()>,
    ) -> StdResult<HostResponse> {
        if contract == self.escrow {
            return Err(error("escrow cannot own tasks"));
        }
        match msg {
            SchedulerMsg::RegisterTask {
                control,
                refund,
                gas_per_poll,
                max_gas_price,
                expires_at,
                funds,
            } => {
                self.validate_coin(&funds)?;
                let control = api.addr_validate(&control)?;
                let refund = api.addr_validate(&refund)?;
                if refund == self.escrow {
                    return Err(error("escrow cannot receive refunds"));
                }
                if gas_per_poll == 0 || gas_per_poll > self.config.max_gas_per_poll {
                    return Err(error("invalid poll gas cap"));
                }
                if expires_at <= height || expires_at - height > self.config.max_lifetime_blocks {
                    return Err(error("invalid task lifetime"));
                }
                let max_attempt_charge = self.attempt_charge(gas_per_poll)?;
                let counter_key = key(b"counter", contract, 0);
                let last: u64 = storage
                    .get(&counter_key)
                    .map(from_json)
                    .transpose()?
                    .unwrap_or(0);
                let task_id = last
                    .checked_add(1)
                    .ok_or_else(|| error("task ID exhausted"))?;
                let status = if max_gas_price < self.config.gas_price {
                    TaskStatus::Paused {
                        reason: PauseReason::PriceLimit,
                        resume: ResumeState::Runnable,
                    }
                } else if funds.amount < max_attempt_charge {
                    TaskStatus::Paused {
                        reason: PauseReason::InsufficientFunds,
                        resume: ResumeState::Runnable,
                    }
                } else {
                    TaskStatus::Runnable
                };
                let task = TaskFunding {
                    task_id,
                    contract: contract.to_string(),
                    control: control.to_string(),
                    refund: refund.to_string(),
                    available: funds.amount,
                    reserved: Uint128::zero(),
                    cumulative_execution_fees: Uint128::zero(),
                    allocated_bytes: 0,
                    storage_lease_paid: Uint128::zero(),
                    gas_per_poll,
                    max_gas_price,
                    max_attempt_charge,
                    expires_at,
                    status,
                };
                let encoded = to_json_binary(&task)?;
                let counter = to_json_binary(&task_id)?;
                transfer(storage, contract, &self.escrow, &funds)?;
                storage.set(&key(b"task", contract, task_id), &encoded);
                storage.set(&counter_key, &counter);
                Ok(HostResponse {
                    events: vec![event("async_registered", &task)
                        .add_attribute("amount", funds.amount)
                        .add_attribute("denom", funds.denom)],
                    data: Some(counter),
                })
            }
            SchedulerMsg::FundTask {
                task_id,
                contributor,
                funds,
            } => {
                self.validate_coin(&funds)?;
                let contributor = api.addr_validate(&contributor)?;
                let mut task = load(storage, contract, task_id)?;
                active(&task, height)?;
                task.available = task
                    .available
                    .checked_add(funds.amount)
                    .map_err(StdError::overflow)?;
                let encoded = to_json_binary(&task)?;
                transfer(storage, contract, &self.escrow, &funds)?;
                storage.set(&key(b"task", contract, task_id), &encoded);
                Ok(HostResponse {
                    events: vec![event("async_funded", &task)
                        .add_attribute("contributor", contributor)
                        .add_attribute("amount", funds.amount)
                        .add_attribute("denom", funds.denom)
                        .add_attribute("available", task.available)],
                    data: None,
                })
            }
            SchedulerMsg::ResumeTask { task_id, actor } => {
                let mut task = load(storage, contract, task_id)?;
                authorize(api, &task, &actor)?;
                active(&task, height)?;
                let TaskStatus::Paused { ref resume, .. } = task.status else {
                    return Err(error("task is not paused"));
                };
                if task.max_gas_price < self.config.gas_price {
                    return Err(error("gas price exceeds task limit"));
                }
                task.max_attempt_charge = self.attempt_charge(task.gas_per_poll)?;
                if task.available < task.max_attempt_charge {
                    return Err(error("insufficient task funds"));
                }
                task.status = match resume {
                    ResumeState::Runnable => TaskStatus::Runnable,
                    ResumeState::Waiting => TaskStatus::Waiting,
                };
                save(storage, contract, &task)?;
                Ok(HostResponse {
                    events: vec![event("async_resumed", &task)],
                    data: None,
                })
            }
            SchedulerMsg::CancelTask { task_id, actor } => {
                let mut task = load(storage, contract, task_id)?;
                authorize(api, &task, &actor)?;
                if matches!(
                    task.status,
                    TaskStatus::Cancelled | TaskStatus::Completed | TaskStatus::Failed
                ) {
                    return Err(error("task already cancelled"));
                }
                // Also permits reclamation after logical expiry, before cleanup.
                let refund = api.addr_validate(&task.refund)?;
                let amount = task.available;
                task.available = Uint128::zero();
                task.status = TaskStatus::Cancelled;
                let encoded = to_json_binary(&task)?;
                if !amount.is_zero() {
                    transfer(
                        storage,
                        &self.escrow,
                        &refund,
                        &Coin::new(amount.u128(), &self.config.denom),
                    )?;
                }
                storage.set(&key(b"task", contract, task_id), &encoded);
                Ok(HostResponse {
                    events: vec![event("async_cancelled", &task).add_attribute("refunded", amount)],
                    data: None,
                })
            }
        }
    }

    pub fn query(
        &self,
        api: &dyn Api,
        storage: &dyn Storage,
        height: u64,
        msg: SchedulerQuery,
    ) -> StdResult<Binary> {
        self.validate_config(false)?;
        self.query_validated(api, storage, height, msg)
    }

    pub(crate) fn query_runtime(
        &self,
        api: &dyn Api,
        storage: &dyn Storage,
        height: u64,
        msg: SchedulerQuery,
    ) -> StdResult<Binary> {
        self.validate_config(true)?;
        self.query_validated(api, storage, height, msg)
    }

    fn query_validated(
        &self,
        api: &dyn Api,
        storage: &dyn Storage,
        height: u64,
        msg: SchedulerQuery,
    ) -> StdResult<Binary> {
        match msg {
            SchedulerQuery::FundingConfig {} => to_json_binary(&self.config),
            SchedulerQuery::TaskFunding { contract, task_id } => {
                let contract = api.addr_validate(&contract)?;
                let mut task = load(storage, &contract, task_id)?;
                task.max_attempt_charge = self.attempt_charge(task.gas_per_poll)?;
                if height >= task.expires_at
                    && !matches!(
                        task.status,
                        TaskStatus::Cancelled | TaskStatus::Completed | TaskStatus::Failed
                    )
                {
                    task.status = TaskStatus::Expired;
                }
                to_json_binary(&task)
            }
        }
    }

    /// Trusted scheduler hook. `resume` describes the saved continuation's
    /// readiness; public funding callers must never supply it.
    pub fn pause(
        &self,
        storage: &mut dyn Storage,
        contract: &Addr,
        task_id: TaskId,
        reason: PauseReason,
        resume: ResumeState,
    ) -> StdResult<()> {
        let mut task = load(storage, contract, task_id)?;
        if matches!(
            task.status,
            TaskStatus::Cancelled
                | TaskStatus::Expired
                | TaskStatus::Completed
                | TaskStatus::Failed
        ) {
            return Err(error("terminal task"));
        }
        task.status = TaskStatus::Paused { reason, resume };
        save(storage, contract, &task)
    }

    fn validate_config(&self, allow_zero_price: bool) -> StdResult<()> {
        if self.config.denom.is_empty()
            || (!allow_zero_price && self.config.gas_price.is_zero())
            || self.config.max_gas_per_poll == 0
            || self.config.max_lifetime_blocks == 0
        {
            return Err(error("invalid scheduler funding configuration"));
        }
        Ok(())
    }
    fn validate_coin(&self, coin: &Coin) -> StdResult<()> {
        if coin.denom != self.config.denom || coin.amount.is_zero() {
            return Err(error("invalid gas deposit"));
        }
        Ok(())
    }
    pub(crate) fn attempt_charge(&self, gas: u64) -> StdResult<Uint128> {
        self.attempt_charge_at(gas, self.config.gas_price)
    }
    pub fn attempt_charge_at(&self, gas: u64, price: Decimal) -> StdResult<Uint128> {
        Uint128::from(gas)
            .checked_mul_ceil(price)
            .map_err(|e| error(e.to_string()))
    }
}

fn error(msg: impl Into<String>) -> StdError {
    StdError::generic_err(msg)
}
fn key(prefix: &[u8], contract: &Addr, task_id: TaskId) -> Vec<u8> {
    // The fixed-width ID suffix and delimited namespace keep keys unambiguous.
    [
        b"native-async/v1/".as_slice(),
        prefix,
        b"/",
        contract.as_bytes(),
        b"/",
        &task_id.to_be_bytes(),
    ]
    .concat()
}
pub(crate) fn load(
    storage: &dyn Storage,
    contract: &Addr,
    task_id: TaskId,
) -> StdResult<TaskFunding> {
    from_json(
        storage
            .get(&key(b"task", contract, task_id))
            .ok_or_else(|| error("unknown task"))?,
    )
}
pub(crate) fn save(
    storage: &mut dyn Storage,
    contract: &Addr,
    task: &TaskFunding,
) -> StdResult<()> {
    storage.set(
        &key(b"task", contract, task.task_id),
        &to_json_binary(task)?,
    );
    Ok(())
}
pub(crate) fn remove(storage: &mut dyn Storage, contract: &Addr, id: u64) {
    storage.remove(&key(b"task", contract, id));
}
fn active(task: &TaskFunding, height: u64) -> StdResult<()> {
    if height >= task.expires_at
        || matches!(
            task.status,
            TaskStatus::Cancelled
                | TaskStatus::Expired
                | TaskStatus::Completed
                | TaskStatus::Failed
        )
    {
        return Err(error("task is terminal or expired"));
    }
    Ok(())
}
fn authorize(api: &dyn Api, task: &TaskFunding, actor: &str) -> StdResult<()> {
    if api.addr_validate(actor)?.as_str() != task.control {
        return Err(error("unauthorized task controller"));
    }
    Ok(())
}
fn event(name: &str, task: &TaskFunding) -> Event {
    Event::new(name)
        .add_attribute("contract", &task.contract)
        .add_attribute("task_id", task.task_id.to_string())
}
