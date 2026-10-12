//! Deterministic host reference scheduler. All user effects must use the storage
//! passed to PollHost; process-local side effects cannot participate in rollback.
use crate::{
    host::{self, Keeper},
    runtime_store::*,
    *,
};
use cosmwasm_std::{
    Addr, Api, Binary, Coin, CosmosMsg, Decimal, Order, StdError, StdResult, Storage, Uint128,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub block_gas: u64,
    pub price_grace_blocks: u64,
    pub price_adjust_denominator: u64,
    pub max_price_pauses: u32,
    pub host_gas: u64,
    /// Separate application-reserved maintenance allocation.
    pub maintenance_gas: u64,
    pub maintenance_gas_per_item: u64,
    pub max_visits: u32,
    pub max_promotions: u32,
    pub max_expirations: u32,
    pub max_prunes: u32,
    pub max_tasks: u64,
    pub max_contract_tasks: u64,
    pub max_allocated_bytes: u64,
    pub max_task_bytes: u64,
    pub max_messages: usize,
    pub max_message_bytes: usize,
    pub lease_price_per_byte_block: Uint128,
    pub cleanup_fee: Uint128,
    pub retention_blocks: u64,
    pub fee_collector: Addr,
}
pub struct Runtime {
    pub keeper: Keeper,
    pub config: RuntimeConfig,
}

/// Values here come from the trusted host meter, never the contract response.
pub struct Metered<T> {
    pub gas_used: u64,
    pub result: Result<T, PauseReason>,
}
pub trait PollHost {
    fn code_checksum(&self, contract: &str) -> StdResult<Binary>;
    fn poll(
        &mut self,
        storage: &mut dyn Storage,
        contract: &str,
        request: &PollRequest,
        gas_limit: u64,
    ) -> StdResult<Metered<PollResponse>>;
    fn dispatch(
        &mut self,
        storage: &mut dyn Storage,
        contract: &str,
        messages: &[CosmosMsg],
        gas_limit: u64,
    ) -> StdResult<Metered<()>>;
    fn transfer(
        &mut self,
        storage: &mut dyn Storage,
        from: &Addr,
        to: &Addr,
        coin: &Coin,
    ) -> StdResult<()>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Runnable,
    Waiting,
    Paused,
    Completed,
    Failed,
    Cancelled,
    Expired,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionTask {
    pub contract: String,
    pub task_id: u64,
    pub program: TaskProgram,
    pub code_checksum: Binary,
    pub state: Option<Binary>,
    pub wait: Option<Wait>,
    pub inbox: Option<Resolution>,
    pub wait_sequence: u64,
    pub status: ExecutionStatus,
    pub allocated_bytes: u64,
    pub storage_fee: Uint128,
    pub charged: Uint128,
    pub reserved: Uint128,
    pub last_attempt: Option<u64>,
    #[serde(default)]
    pub ready_at: u64,
    pub output: Option<Binary>,
    pub error: Option<String>,
    pub expires_at: u64,
    pub retain_until: u64,
    queue_key: Option<Binary>,
    expiry_key: Binary,
}
#[derive(Default, Debug, PartialEq)]
pub struct BlockReport {
    pub base_price: Decimal,
    pub next_base_price: Decimal,
    /// Metered execution gas, excluding the fixed host accounting charge.
    pub poll_gas_used: u64,
    pub polls: u32,
    pub visits: u32,
    pub promotions: u32,
    pub price_pauses: u32,
    pub expirations: u32,
    pub prunes: u32,
    pub gas_used: u64,
    pub maintenance_gas_used: u64,
    /// Terminal refunds the refund address rejected, paid instead to the
    /// controller or, failing that, the owning contract.
    pub redirected_refunds: u32,
    /// Terminal transitions whose refund all three recipients rejected; the
    /// escrow stays in the escrow account rather than halting the block.
    pub stranded_refunds: u32,
}
#[derive(Serialize, Deserialize)]
struct Reference {
    contract: String,
    id: u64,
}
fn err(s: impl Into<String>) -> StdError {
    StdError::generic_err(s)
}
fn next(h: u64) -> StdResult<u64> {
    h.checked_add(1).ok_or_else(|| err("height overflow"))
}
fn task_key(c: &str, id: u64) -> Vec<u8> {
    [
        b"na2/task/".as_slice(),
        c.as_bytes(),
        b"/",
        &id.to_be_bytes(),
    ]
    .concat()
}
fn count_key(c: &str) -> Vec<u8> {
    [b"na2/count/".as_slice(), c.as_bytes()].concat()
}
fn bid_key(bid: Decimal, h: u64, seq: u64) -> Vec<u8> {
    [
        b"na2/bid/".as_slice(),
        bid_component(bid).as_bytes(),
        &h.to_be_bytes(),
        &seq.to_be_bytes(),
    ]
    .concat()
}
fn save(s: &mut dyn Storage, t: &ExecutionTask) -> StdResult<()> {
    put(s, &task_key(&t.contract, t.task_id), t)
}
fn load(s: &dyn Storage, c: &str, id: u64) -> StdResult<ExecutionTask> {
    get(s, &task_key(c, id))
}
fn terminal(t: &ExecutionTask) -> bool {
    matches!(
        t.status,
        ExecutionStatus::Completed
            | ExecutionStatus::Failed
            | ExecutionStatus::Cancelled
            | ExecutionStatus::Expired
    )
}
fn unqueue(s: &mut dyn Storage, t: &mut ExecutionTask) {
    if let Some(k) = t.queue_key.take() {
        s.remove(k.as_slice());
    }
}
fn timed(s: &mut dyn Storage, prefix: &[u8], height: u64, t: &ExecutionTask) -> StdResult<Vec<u8>> {
    let seq = bump(s, b"na2/sequence")?;
    let k = [prefix, &height.to_be_bytes(), &seq.to_be_bytes()].concat();
    put(
        s,
        &k,
        &Reference {
            contract: t.contract.clone(),
            id: t.task_id,
        },
    )?;
    Ok(k)
}
fn schedule(s: &mut dyn Storage, t: &mut ExecutionTask, h: u64) -> StdResult<()> {
    unqueue(s, t);
    t.queue_key = Some(timed(s, b"na2/due/", h, t)?.into());
    Ok(())
}
fn ready(s: &mut dyn Storage, t: &mut ExecutionTask, bid: Decimal, h: u64) -> StdResult<()> {
    let seq = bump(s, b"na2/sequence")?;
    ready_with_sequence(s, t, bid, h, seq)
}
fn ready_with_sequence(
    s: &mut dyn Storage,
    t: &mut ExecutionTask,
    bid: Decimal,
    h: u64,
    seq: u64,
) -> StdResult<()> {
    unqueue(s, t);
    let k = bid_key(bid, h, seq);
    put(
        s,
        &k,
        &Reference {
            contract: t.contract.clone(),
            id: t.task_id,
        },
    )?;
    t.queue_key = Some(k.into());
    t.status = ExecutionStatus::Runnable;
    t.ready_at = h;
    Ok(())
}
fn due(s: &dyn Storage, prefix: &[u8], h: u64) -> Option<(Vec<u8>, Vec<u8>)> {
    first(s, prefix).filter(|(k, _)| {
        u64::from_be_bytes(k[prefix.len()..prefix.len() + 8].try_into().unwrap()) <= h
    })
}

impl Runtime {
    pub fn base_price(&self, s: &dyn Storage) -> StdResult<Decimal> {
        if self.config.block_gas / 2 == 0 || self.config.price_adjust_denominator == 0 {
            return Err(err("invalid runtime price configuration"));
        }
        s.get(b"na2/base-price")
            .map(cosmwasm_std::from_json)
            .transpose()
            .map(|price| price.unwrap_or(self.keeper.config.gas_price))
    }

    fn live_keeper(&self, s: &dyn Storage) -> StdResult<Keeper> {
        let mut config = self.keeper.config.clone();
        config.gas_price = self.base_price(s)?;
        config.min_base_price = self.keeper.config.gas_price;
        config.target_gas = self.config.block_gas / 2;
        config.price_grace_blocks = self.config.price_grace_blocks;
        config.last_block_poll_gas_used = number(s, b"na2/last-poll-gas")?;
        Ok(Keeper {
            config,
            escrow: self.keeper.escrow.clone(),
        })
    }

    fn maintenance(&self, report: &mut BlockReport) -> bool {
        if self.config.maintenance_gas_per_item == 0
            || self.config.maintenance_gas_per_item
                > self
                    .config
                    .maintenance_gas
                    .saturating_sub(report.maintenance_gas_used)
        {
            return false;
        }
        report.maintenance_gas_used += self.config.maintenance_gas_per_item;
        true
    }
    /// Reserve maximum-width accounting, lifecycle and queue metadata, rather
    /// than today's encoding. Mandatory settlement and pause recording must
    /// never become quota failures after an admitted poll has spent gas.
    fn check_quota(&self, s: &dyn Storage, t: &ExecutionTask) -> StdResult<()> {
        let mut funding = host::load(s, &Addr::unchecked(&t.contract), t.task_id)?;
        let inbox_reserve = match t.wait {
            Some(Wait::Operation {
                max_result_bytes, ..
            }) => crate::inbox_reserve(max_result_bytes),
            _ => 0,
        };
        let mut reserved = t.clone();
        reserved.wait_sequence = u64::MAX;
        reserved.last_attempt = Some(u64::MAX);
        reserved.ready_at = u64::MAX;
        reserved.charged = Uint128::MAX;
        reserved.reserved = Uint128::MAX;
        reserved.storage_fee = Uint128::MAX;
        reserved.allocated_bytes = u64::MAX;
        reserved.expires_at = u64::MAX;
        reserved.retain_until = u64::MAX;
        reserved.status = ExecutionStatus::Completed; // longest execution status
        reserved.queue_key = Some(vec![0; b"na2/bid/".len() + 32 + 16].into());
        if inbox_reserve != 0 {
            reserved.inbox = None;
        }
        funding.available = Uint128::MAX;
        funding.reserved = Uint128::MAX;
        funding.cumulative_execution_fees = Uint128::MAX;
        funding.max_attempt_charge = Uint128::MAX;
        funding.storage_lease_paid = Uint128::MAX;
        funding.allocated_bytes = u64::MAX;
        funding.expires_at = u64::MAX;
        funding.status = TaskStatus::Paused {
            reason: PauseReason::InsufficientFunds,
            resume: ResumeState::Runnable,
        };
        let used = cosmwasm_std::to_json_vec(&reserved)?.len() as u64
            + cosmwasm_std::to_json_vec(&funding)?.len() as u64
            + crate::METADATA_ALLOWANCE
            + inbox_reserve;
        if used > t.allocated_bytes {
            return Err(err("encoded task and inbox exceed storage allocation"));
        }
        Ok(())
    }

    /// The adapter authenticates the owning contract; direct account custom
    /// messages must not call this router.
    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &self,
        api: &dyn Api,
        s: &mut dyn Storage,
        h: u64,
        c: &Addr,
        msg: RuntimeMsg,
        scheduler_phase: bool,
        host: &mut impl PollHost,
    ) -> StdResult<host::HostResponse> {
        let data = match msg {
            RuntimeMsg::Register {
                control,
                refund,
                gas_per_poll,
                max_gas_price,
                expires_at,
                funds,
                program,
                allocated_bytes,
            } => Some(cosmwasm_std::to_json_binary(&self.register(
                api,
                s,
                h,
                c,
                SchedulerMsg::RegisterTask {
                    control,
                    refund,
                    gas_per_poll,
                    max_gas_price,
                    expires_at,
                    funds,
                },
                program,
                allocated_bytes,
                host,
            )?)?),
            RuntimeMsg::Deliver {
                task_id,
                wait_sequence,
                resolver,
                resolution,
            } => {
                self.deliver(
                    s,
                    h,
                    c,
                    task_id,
                    wait_sequence,
                    &resolver,
                    resolution,
                    scheduler_phase,
                )?;
                None
            }
            RuntimeMsg::Funding(msg) => {
                return self.manage(api, s, h, c, msg, host);
            }
            msg @ (RuntimeMsg::ExtendLease { .. } | RuntimeMsg::UpdateBudget { .. }) => {
                self.update_policy(api, s, h, c, msg, host)?;
                None
            }
        };
        Ok(host::HostResponse {
            events: vec![],
            data,
        })
    }
    pub fn query(
        &self,
        api: &dyn Api,
        s: &dyn Storage,
        h: u64,
        msg: RuntimeQuery,
    ) -> StdResult<Binary> {
        match msg {
            RuntimeQuery::Funding(msg) => self.live_keeper(s)?.query_runtime(api, s, h, msg),
            RuntimeQuery::Task { contract, task_id } => {
                let task = self.task(s, &api.addr_validate(&contract)?, task_id, h)?;
                let funding: TaskFunding =
                    cosmwasm_std::from_json(self.live_keeper(s)?.query_runtime(
                        api,
                        s,
                        h,
                        SchedulerQuery::TaskFunding { contract, task_id },
                    )?)?;
                cosmwasm_std::to_json_binary(&TaskResponse {
                    contract: task.contract,
                    task_id: task.task_id,
                    program: task.program,
                    code_checksum: task.code_checksum,
                    status: funding.status,
                    wait_sequence: task.wait_sequence,
                    state: task.state,
                    wait: task.wait,
                    inbox: task.inbox,
                    output: task.output,
                    error: task.error,
                    last_attempt: task.last_attempt,
                    expires_at: task.expires_at,
                    retain_until: task.retain_until,
                    allocated_bytes: task.allocated_bytes,
                })
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn update_policy(
        &self,
        api: &dyn Api,
        s: &mut dyn Storage,
        h: u64,
        c: &Addr,
        msg: RuntimeMsg,
        host: &mut impl PollHost,
    ) -> StdResult<()> {
        let (id, actor) = match &msg {
            RuntimeMsg::ExtendLease { task_id, actor, .. }
            | RuntimeMsg::UpdateBudget { task_id, actor, .. } => (*task_id, actor),
            _ => return Err(err("unsupported policy update")),
        };
        let mut cache = Cache::new(s);
        let base_price = self.base_price(&cache)?;
        let mut t = load(&cache, c.as_str(), id)?;
        let mut f = host::load(&cache, c, id)?;
        if api.addr_validate(actor)?.as_str() != f.control {
            return Err(err("unauthorized task controller"));
        }
        if terminal(&t) || h >= t.expires_at {
            return Err(err("task is terminal or expired"));
        }
        if !t.reserved.is_zero() || !f.reserved.is_zero() {
            return Err(err("cannot manage an in-flight poll"));
        }
        match msg {
            RuntimeMsg::ExtendLease {
                allocated_bytes,
                expires_at,
                ..
            } => {
                if allocated_bytes < t.allocated_bytes
                    || expires_at < t.expires_at
                    || (allocated_bytes == t.allocated_bytes && expires_at == t.expires_at)
                    || allocated_bytes > self.config.max_task_bytes
                    || expires_at - h > self.keeper.config.max_lifetime_blocks
                {
                    return Err(err("invalid lease extension"));
                }
                let retain_until = expires_at
                    .checked_add(self.config.retention_blocks)
                    .ok_or_else(|| err("retention overflow"))?;
                let added_bytes = allocated_bytes - t.allocated_bytes;
                let used = number(&cache, b"na2/bytes")?
                    .checked_add(added_bytes)
                    .ok_or_else(|| err("storage overflow"))?;
                if used > self.config.max_allocated_bytes {
                    return Err(err("runtime capacity exhausted"));
                }
                let old_retention = t
                    .expires_at
                    .checked_add(self.config.retention_blocks)
                    .ok_or_else(|| err("retention overflow"))?;
                let byte_blocks = Uint128::from(added_bytes)
                    .checked_mul(Uint128::from(old_retention - h))?
                    .checked_add(
                        Uint128::from(allocated_bytes)
                            .checked_mul(Uint128::from(expires_at - t.expires_at))?,
                    )?;
                let rent = byte_blocks.checked_mul(self.config.lease_price_per_byte_block)?;
                f.available = f.available.checked_sub(rent)?;
                f.storage_lease_paid = f.storage_lease_paid.checked_add(rent)?;
                f.allocated_bytes = allocated_bytes;
                f.expires_at = expires_at;
                t.storage_fee = t.storage_fee.checked_add(rent)?;
                t.allocated_bytes = allocated_bytes;
                t.expires_at = expires_at;
                t.retain_until = retain_until;
                cache.remove(t.expiry_key.as_slice());
                t.expiry_key = timed(&mut cache, b"na2/expiry/", expires_at, &t)?.into();
                put(&mut cache, b"na2/bytes", &used)?;
                if !rent.is_zero() {
                    host.transfer(
                        &mut cache,
                        &self.keeper.escrow,
                        &self.config.fee_collector,
                        &Coin::new(rent.u128(), &self.keeper.config.denom),
                    )?;
                }
            }
            RuntimeMsg::UpdateBudget {
                gas_per_poll,
                max_gas_price,
                refund,
                ..
            } => {
                let refund = api.addr_validate(&refund)?;
                if refund == self.keeper.escrow {
                    return Err(err("escrow cannot receive refunds"));
                }
                if gas_per_poll <= self.config.host_gas
                    || gas_per_poll > self.config.block_gas
                    || gas_per_poll > self.keeper.config.max_gas_per_poll
                {
                    return Err(err("invalid poll gas cap"));
                }
                f.gas_per_poll = gas_per_poll;
                f.max_gas_price = max_gas_price;
                f.max_attempt_charge = self.keeper.attempt_charge_at(gas_per_poll, base_price)?;
                f.refund = refund.to_string();
                if let Some(key) = t
                    .queue_key
                    .as_ref()
                    .filter(|key| key.as_slice().starts_with(b"na2/bid/"))
                {
                    // Budget changes retain both readiness age and the original
                    // sequence, including when joining an existing bid tier.
                    let seq = u64::from_be_bytes(key[key.len() - 8..].try_into().unwrap());
                    let value = cache
                        .get(key.as_slice())
                        .ok_or_else(|| err("missing bid entry"))?;
                    let new_key = bid_key(max_gas_price, t.ready_at, seq);
                    cache.remove(key.as_slice());
                    cache.set(&new_key, &value);
                    t.queue_key = Some(new_key.into());
                }
            }
            _ => unreachable!(),
        }
        host::save(&mut cache, c, &f)?;
        self.check_quota(&cache, &t)?;
        save(&mut cache, &t)?;
        cache.commit();
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub fn register(
        &self,
        api: &dyn Api,
        storage: &mut dyn Storage,
        height: u64,
        contract: &Addr,
        funding: SchedulerMsg,
        program: TaskProgram,
        allocated_bytes: u64,
        host: &mut impl PollHost,
    ) -> StdResult<u64> {
        let base_price = self.base_price(storage)?;
        let SchedulerMsg::RegisterTask {
            expires_at,
            gas_per_poll,
            max_gas_price,
            funds: deposit,
            ..
        } = &funding
        else {
            return Err(err("registration funding required"));
        };
        if *max_gas_price < self.keeper.config.gas_price {
            return Err(err("bid below minimum base price"));
        }
        if allocated_bytes == 0
            || self.config.host_gas == 0
            || self.config.maintenance_gas_per_item == 0
            || allocated_bytes > self.config.max_task_bytes
            || program.kind.is_empty()
            || program.kind.len() > 128
            || program.input.len() as u64 > allocated_bytes
            || *gas_per_poll <= self.config.host_gas
            || *gas_per_poll > self.config.block_gas
        {
            return Err(err("invalid runtime admission"));
        }
        let expiry = *expires_at;
        let lease_blocks = expiry
            .checked_sub(height)
            .ok_or_else(|| err("invalid lease"))?;
        let retain_until = expiry
            .checked_add(self.config.retention_blocks)
            .ok_or_else(|| err("retention overflow"))?;
        let used = number(storage, b"na2/bytes")?
            .checked_add(allocated_bytes)
            .ok_or_else(|| err("storage overflow"))?;
        if used > self.config.max_allocated_bytes
            || number(storage, b"na2/count")? >= self.config.max_tasks
            || number(storage, &count_key(contract.as_str()))? >= self.config.max_contract_tasks
        {
            return Err(err("runtime capacity exhausted"));
        }
        let storage_fee = self
            .config
            .lease_price_per_byte_block
            .checked_mul(Uint128::from(allocated_bytes))?
            .checked_mul(Uint128::from(
                lease_blocks
                    .checked_add(self.config.retention_blocks)
                    .ok_or_else(|| err("lease overflow"))?,
            ))?
            .checked_add(self.config.cleanup_fee)?;
        let attempt_charge = self.keeper.attempt_charge_at(*gas_per_poll, base_price)?;
        if deposit.amount < storage_fee.checked_add(attempt_charge)? {
            return Err(err("insufficient initial task funding"));
        }
        let code_checksum = host.code_checksum(contract.as_str())?;
        if code_checksum.is_empty() {
            return Err(err("missing code identity"));
        }
        let mut cache = Cache::new(storage);
        let response = self.keeper.execute_runtime(
            api,
            &mut cache,
            height,
            contract,
            funding,
            |s, f, t, c| host.transfer(s, f, t, c),
        )?;
        let id = cosmwasm_std::from_json(response.data.ok_or_else(|| err("missing task ID"))?)?;
        let mut funds = host::load(&cache, contract, id)?;
        funds.available = funds.available.checked_sub(storage_fee)?;
        funds.allocated_bytes = allocated_bytes;
        funds.storage_lease_paid = storage_fee;
        funds.max_attempt_charge = attempt_charge;
        if !storage_fee.is_zero() {
            host.transfer(
                &mut cache,
                &self.keeper.escrow,
                &self.config.fee_collector,
                &Coin::new(storage_fee.u128(), &self.keeper.config.denom),
            )?;
        }
        host::save(&mut cache, contract, &funds)?;
        let mut task = ExecutionTask {
            contract: contract.to_string(),
            task_id: id,
            program,
            code_checksum,
            state: None,
            wait: None,
            inbox: None,
            wait_sequence: 0,
            status: if funds.status == TaskStatus::Runnable {
                ExecutionStatus::Runnable
            } else {
                ExecutionStatus::Paused
            },
            allocated_bytes,
            storage_fee,
            charged: Uint128::zero(),
            reserved: Uint128::zero(),
            last_attempt: None,
            ready_at: 0,
            output: None,
            error: None,
            expires_at: expiry,
            retain_until,
            queue_key: None,
            expiry_key: Binary::default(),
        };
        task.expiry_key = timed(&mut cache, b"na2/expiry/", expiry, &task)?.into();
        if task.status == ExecutionStatus::Runnable {
            schedule(&mut cache, &mut task, next(height)?)?;
        }
        self.check_quota(&cache, &task)?;
        save(&mut cache, &task)?;
        put(&mut cache, b"na2/bytes", &used)?;
        bump(&mut cache, b"na2/count")?;
        bump(&mut cache, &count_key(contract.as_str()))?;
        cache.commit();
        Ok(id)
    }

    pub fn task(&self, s: &dyn Storage, c: &Addr, id: u64, h: u64) -> StdResult<ExecutionTask> {
        let mut t = load(s, c.as_str(), id)?;
        if !terminal(&t) && h >= t.expires_at {
            t.status = ExecutionStatus::Expired;
        }
        Ok(t)
    }
    pub fn assert_migration_allowed(&self, s: &dyn Storage, c: &Addr) -> StdResult<()> {
        if number(s, &count_key(c.as_str()))? != 0 {
            return Err(err("active tasks block migration"));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn deliver(
        &self,
        s: &mut dyn Storage,
        h: u64,
        c: &Addr,
        id: u64,
        sequence: u64,
        resolver: &str,
        resolution: Resolution,
        scheduler_phase: bool,
    ) -> StdResult<()> {
        let mut cache = Cache::new(s);
        self.deliver_cached(
            &mut cache,
            h,
            c,
            id,
            sequence,
            resolver,
            resolution,
            scheduler_phase,
        )?;
        cache.commit();
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn deliver_cached(
        &self,
        s: &mut dyn Storage,
        h: u64,
        c: &Addr,
        id: u64,
        sequence: u64,
        resolver: &str,
        resolution: Resolution,
        scheduler_phase: bool,
    ) -> StdResult<()> {
        let mut t = load(s, c.as_str(), id)?;
        if terminal(&t) || h >= t.expires_at || t.wait_sequence != sequence || t.inbox.is_some() {
            return Err(err("inactive or duplicate resolution"));
        }
        let Some(Wait::Operation {
            resolver: expected,
            deadline,
            max_result_bytes,
        }) = &t.wait
        else {
            return Err(err("not waiting for operation"));
        };
        let bytes = match &resolution {
            Resolution::Success { data } => data.len(),
            Resolution::Error { code, message } => code
                .len()
                .checked_add(message.len())
                .ok_or_else(|| err("result overflow"))?,
            _ => return Err(err("only host may supply timer/timeout")),
        };
        if resolver != expected || h >= *deadline || bytes > *max_result_bytes as usize {
            return Err(err("unauthorized, late, or oversized result"));
        }
        t.inbox = Some(resolution);
        if t.status != ExecutionStatus::Paused {
            if scheduler_phase {
                t.status = ExecutionStatus::Runnable;
                schedule(s, &mut t, next(h)?)?;
            } else {
                let funding = host::load(s, c, id)?;
                ready(s, &mut t, funding.max_gas_price, h)?;
            }
            // Keep the public status in step with the Go adapter's delivery.
            let mut funding = host::load(s, c, id)?;
            funding.status = TaskStatus::Runnable;
            host::save(s, c, &funding)?;
        }
        self.check_quota(s, &t)?;
        save(s, &t)
    }

    pub fn manage(
        &self,
        api: &dyn Api,
        s: &mut dyn Storage,
        h: u64,
        c: &Addr,
        msg: SchedulerMsg,
        host: &mut impl PollHost,
    ) -> StdResult<host::HostResponse> {
        let (id, resume, cancel) = match &msg {
            SchedulerMsg::FundTask { task_id, .. } => (*task_id, false, false),
            SchedulerMsg::ResumeTask { task_id, .. } => (*task_id, true, false),
            SchedulerMsg::CancelTask { task_id, .. } => (*task_id, false, true),
            _ => return Err(err("use executable registration")),
        };
        let mut cache = Cache::new(s);
        let keeper = self.live_keeper(&cache)?;
        let mut t = load(&cache, c.as_str(), id)?;
        if !t.reserved.is_zero() && (resume || cancel) {
            return Err(err("cannot manage an in-flight poll"));
        }
        let response = keeper.execute_runtime(api, &mut cache, h, c, msg, |s, f, t, c| {
            host.transfer(s, f, t, c)
        })?;
        if cancel {
            self.finish(&mut cache, &mut t, ExecutionStatus::Cancelled, host)?;
        } else if resume {
            if t.inbox.is_some() || t.wait.is_none() {
                t.status = ExecutionStatus::Runnable;
                schedule(&mut cache, &mut t, next(h)?)?;
            } else {
                t.status = ExecutionStatus::Waiting;
                let due = match t.wait.as_ref().unwrap() {
                    Wait::Height { height } => *height,
                    Wait::Operation { deadline, .. } => *deadline,
                };
                schedule(&mut cache, &mut t, due.max(next(h)?))?;
            }
        }
        if resume {
            let mut funding = host::load(&cache, c, id)?;
            funding.status = if t.status == ExecutionStatus::Waiting {
                TaskStatus::Waiting
            } else {
                TaskStatus::Runnable
            };
            host::save(&mut cache, c, &funding)?;
        }
        if !cancel {
            self.check_quota(&cache, &t)?;
        }
        save(&mut cache, &t)?;
        cache.commit();
        Ok(response)
    }

    /// Scheduler-driven terminal transitions must not fail the block. A refund
    /// the bank rejects goes to the controller, then to the owning contract;
    /// if all three reject it, it is left in escrow and counted instead.
    fn settle(
        &self,
        s: &mut dyn Storage,
        t: &mut ExecutionTask,
        status: ExecutionStatus,
        host: &mut impl PollHost,
        report: &mut BlockReport,
    ) -> StdResult<()> {
        let f = host::load(s, &Addr::unchecked(&t.contract), t.task_id)?;
        let mut tried: Vec<String> = Vec::new();
        for recipient in [f.refund.clone(), f.control, t.contract.clone()] {
            if tried.contains(&recipient) {
                continue;
            }
            let mut attempt = t.clone();
            let mut cache = Cache::new(s);
            if self
                .finish_with(
                    &mut cache,
                    &mut attempt,
                    status.clone(),
                    host,
                    Some(&recipient),
                )
                .is_ok()
            {
                cache.commit();
                *t = attempt;
                if recipient != f.refund {
                    report.redirected_refunds += 1;
                }
                return Ok(());
            }
            tried.push(recipient);
        }
        report.stranded_refunds += 1;
        self.finish_with(s, t, status, host, None)
    }

    fn finish(
        &self,
        s: &mut dyn Storage,
        t: &mut ExecutionTask,
        status: ExecutionStatus,
        host: &mut impl PollHost,
    ) -> StdResult<()> {
        let refund = host::load(s, &Addr::unchecked(&t.contract), t.task_id)?.refund;
        self.finish_with(s, t, status, host, Some(&refund))
    }

    fn finish_with(
        &self,
        s: &mut dyn Storage,
        t: &mut ExecutionTask,
        status: ExecutionStatus,
        host: &mut impl PollHost,
        recipient: Option<&str>,
    ) -> StdResult<()> {
        if terminal(t) {
            return Err(err("already terminal"));
        }
        unqueue(s, t);
        s.remove(t.expiry_key.as_slice());
        t.status = status;
        t.wait = None;
        t.inbox = None;
        t.state = None;
        t.program.input = Binary::default();
        let c = Addr::unchecked(&t.contract);
        let mut f = host::load(s, &c, t.task_id)?;
        if let Some(recipient) = recipient.filter(|_| !f.available.is_zero()) {
            host.transfer(
                s,
                &self.keeper.escrow,
                &Addr::unchecked(recipient),
                &Coin::new(f.available.u128(), &self.keeper.config.denom),
            )?;
        }
        // An unrefunded balance stays in the escrow account (surplus backing).
        f.available = Uint128::zero();
        f.status = match t.status {
            ExecutionStatus::Completed => TaskStatus::Completed,
            ExecutionStatus::Failed => TaskStatus::Failed,
            ExecutionStatus::Cancelled => TaskStatus::Cancelled,
            _ => TaskStatus::Expired,
        };
        host::save(s, &c, &f)?;
        for key in [b"na2/count".to_vec(), count_key(&t.contract)] {
            let n = number(s, &key)?
                .checked_sub(1)
                .ok_or_else(|| err("task count underflow"))?;
            if n == 0 && key == count_key(&t.contract) {
                s.remove(&key);
            } else {
                put(s, &key, &n)?;
            }
        }
        timed(s, b"na2/prune/", t.retain_until, t)?;
        Ok(())
    }

    fn stage(
        &self,
        s: &mut dyn Storage,
        t: &mut ExecutionTask,
        h: u64,
        response: &PollResponse,
    ) -> StdResult<()> {
        if response.messages.len() > self.config.max_messages
            || cosmwasm_std::to_json_vec(&response.messages)?.len() > self.config.max_message_bytes
        {
            return Err(err("outbound limits"));
        }
        t.inbox = None;
        t.wait = None;
        t.program.input = Binary::default();
        match &response.outcome {
            PollOutcome::Pending { state, wait } => {
                let (due, reserved) = match wait {
                    Wait::Height { height } => (*height, 0),
                    Wait::Operation {
                        resolver,
                        deadline,
                        max_result_bytes,
                    } => {
                        if resolver.is_empty() || resolver.len() > 256 {
                            return Err(err("invalid resolver"));
                        }
                        (*deadline, u64::from(*max_result_bytes))
                    }
                };
                if due <= h || state.len() as u64 + reserved > t.allocated_bytes {
                    return Err(err("invalid wait or storage quota"));
                }
                t.wait_sequence = t
                    .wait_sequence
                    .checked_add(1)
                    .ok_or_else(|| err("wait sequence exhausted"))?;
                t.state = Some(state.clone());
                t.wait = Some(wait.clone());
                t.status = ExecutionStatus::Waiting;
                schedule(s, t, due)?;
            }
            PollOutcome::Yield { state } => {
                if state.len() as u64 > t.allocated_bytes {
                    return Err(err("storage quota"));
                }
                t.state = Some(state.clone());
                t.status = ExecutionStatus::Runnable;
                schedule(s, t, next(h)?)?;
            }
            PollOutcome::Complete { output } => {
                if output.len() as u64 > t.allocated_bytes {
                    return Err(err("output quota"));
                }
                t.output = Some(output.clone());
            }
            PollOutcome::Fail { error } => {
                if error.len() as u64 > t.allocated_bytes {
                    return Err(err("error quota"));
                }
                t.error = Some(error.clone());
            }
        }
        self.check_quota(s, t)?;
        save(s, t)
    }

    pub fn run_block(
        &self,
        s: &mut dyn Storage,
        h: u64,
        host: &mut impl PollHost,
    ) -> StdResult<BlockReport> {
        let mut cache = Cache::new(s);
        let report = self.run_block_cached(&mut cache, h, host)?;
        cache.commit();
        Ok(report)
    }
    fn run_block_cached(
        &self,
        s: &mut dyn Storage,
        h: u64,
        host: &mut impl PollHost,
    ) -> StdResult<BlockReport> {
        let base_price = self.base_price(s)?;
        // One invocation per height, persisted to survive host recreation/replay.
        if s.get(b"na2/last-block").is_some() && number(s, b"na2/last-block")? >= h {
            return Ok(BlockReport::default());
        }
        let mut report = BlockReport {
            base_price,
            next_base_price: base_price,
            ..BlockReport::default()
        };
        for _ in 0..self.config.max_expirations {
            if !self.maintenance(&mut report) {
                break;
            }
            let Some((key, value)) = due(s, b"na2/expiry/", h) else {
                break;
            };
            let r: Reference = cosmwasm_std::from_json(value)?;
            let mut cache = Cache::new(s);
            cache.remove(&key);
            let mut t = load(&cache, &r.contract, r.id)?;
            if !terminal(&t) {
                self.settle(
                    &mut cache,
                    &mut t,
                    ExecutionStatus::Expired,
                    host,
                    &mut report,
                )?;
                save(&mut cache, &t)?;
            }
            cache.commit();
            report.expirations += 1;
        }
        for _ in 0..self.config.max_prunes {
            if !self.maintenance(&mut report) {
                break;
            }
            let Some((key, value)) = due(s, b"na2/prune/", h) else {
                break;
            };
            let r: Reference = cosmwasm_std::from_json(value)?;
            let t = load(s, &r.contract, r.id)?;
            let used = number(s, b"na2/bytes")?
                .checked_sub(t.allocated_bytes)
                .ok_or_else(|| err("bytes underflow"))?;
            put(s, b"na2/bytes", &used)?;
            s.remove(&task_key(&r.contract, r.id));
            host::remove(s, &Addr::unchecked(&r.contract), r.id);
            s.remove(&key);
            report.prunes += 1;
        }
        for _ in 0..self.config.max_promotions {
            if !self.maintenance(&mut report) {
                break;
            }
            let Some((key, value)) = due(s, b"na2/due/", h) else {
                break;
            };
            let r: Reference = cosmwasm_std::from_json(value)?;
            let mut t = load(s, &r.contract, r.id)?;
            let ready_at = u64::from_be_bytes(
                key[b"na2/due/".len()..b"na2/due/".len() + 8]
                    .try_into()
                    .unwrap(),
            );
            let seq = u64::from_be_bytes(key[key.len() - 8..].try_into().unwrap());
            s.remove(&key);
            if !terminal(&t) && t.status != ExecutionStatus::Paused && h < t.expires_at {
                let f = host::load(s, &Addr::unchecked(&r.contract), r.id)?;
                // Promotion preserves the original queue position even when
                // newer deliveries already entered the same readiness height.
                ready_with_sequence(s, &mut t, f.max_gas_price, ready_at, seq)?;
            } else {
                t.queue_key = None;
            }
            save(s, &t)?;
            report.promotions += 1;
        }
        // The reverse walk inspects at most max_price_pauses entries, including
        // younger tasks it skips. Each inspection shares the maintenance meter.
        let mut price_cursor = b"na2/bid0".to_vec();
        for _ in 0..self.config.max_price_pauses {
            if !self.maintenance(&mut report) {
                break;
            }
            let Some((key, value)) = s
                .range(Some(b"na2/bid/"), Some(&price_cursor), Order::Descending)
                .next()
            else {
                break;
            };
            price_cursor = key.clone(); // exclusive upper bound; iterator closed before writes
            let r: Reference = cosmwasm_std::from_json(value)?;
            let mut t = load(s, &r.contract, r.id)?;
            let addr = Addr::unchecked(&r.contract);
            let mut f = host::load(s, &addr, r.id)?;
            if f.max_gas_price >= base_price {
                break;
            }
            if h >= t.expires_at
                || h.saturating_sub(t.ready_at.max(t.last_attempt.unwrap_or(0)))
                    < self.config.price_grace_blocks
            {
                continue;
            }
            unqueue(s, &mut t);
            t.status = ExecutionStatus::Paused;
            f.status = TaskStatus::Paused {
                reason: PauseReason::PriceLimit,
                resume: ResumeState::Runnable,
            };
            save(s, &t)?;
            host::save(s, &addr, &f)?;
            report.price_pauses += 1;
        }
        // A single lexical cursor and visit counter cover the entire block.
        // Oversized entries remain persisted but cannot be revisited this block.
        let mut cursor = b"na2/bid/".to_vec();
        while report.visits < self.config.max_visits {
            if !self.maintenance(&mut report) {
                break;
            }
            let Some((ready_key, value)) = s
                .range(Some(&cursor), Some(b"na2/bid0"), Order::Ascending)
                .next()
            else {
                break;
            };
            report.visits += 1;
            cursor = ready_key.clone();
            cursor.push(0); // exclusive lower bound; iterator is closed before writes
            let r: Reference = cosmwasm_std::from_json(value)?;
            let c = r.contract;
            let mut t = load(s, &c, r.id)?;
            let addr = Addr::unchecked(&c);
            let mut f = host::load(s, &addr, t.task_id)?;
            if h >= t.expires_at {
                s.remove(&ready_key);
                t.queue_key = None;
                save(s, &t)?;
                continue;
            }
            // Descending bid order makes every subsequent task ineligible too.
            // Leave priced-out work queued for the separate grace pause pass.
            if f.max_gas_price < base_price {
                break;
            }
            if f.gas_per_poll > self.config.block_gas - report.gas_used {
                continue;
            }
            s.remove(&ready_key);
            t.queue_key = None;
            if t.last_attempt == Some(h) {
                schedule(s, &mut t, next(h)?)?;
                save(s, &t)?;
                continue;
            }
            // A price increase can make an admitted task's next reservation
            // exceed the Uint128 coin domain. No valid escrow can afford that
            // charge; pause it like any other insufficient-funds task.
            let charge = self.keeper.attempt_charge_at(f.gas_per_poll, base_price);
            let blocked = match &charge {
                Ok(charge) if f.available >= *charge => None,
                _ => Some(PauseReason::InsufficientFunds),
            };
            if let Some(reason) = blocked {
                t.status = ExecutionStatus::Paused;
                f.status = TaskStatus::Paused {
                    reason,
                    resume: ResumeState::Runnable,
                };
                save(s, &t)?;
                host::save(s, &addr, &f)?;
            } else {
                let charge = charge?; // Only the affordable, representable branch reaches here.
                t.last_attempt = Some(h);
                t.reserved = charge;
                f.available = f.available.checked_sub(charge)?;
                f.reserved = charge;
                host::save(s, &addr, &f)?;
                save(s, &t)?;
                let step = match &t.state {
                    None => PollStep::Start {
                        input: t.program.input.clone(),
                    },
                    Some(state) => PollStep::Resume {
                        state: state.clone(),
                        resolution: t.inbox.clone().unwrap_or(match t.wait {
                            Some(Wait::Operation { .. }) => Resolution::Timeout {},
                            _ => Resolution::Timer {},
                        }),
                    },
                };
                let request = PollRequest {
                    task_id: t.task_id,
                    wait_sequence: t.wait_sequence,
                    kind: t.program.kind.clone(),
                    version: t.program.version,
                    step,
                };
                let mut cache = Cache::new(s);
                let mut gas = self.config.host_gas;
                let result = if host.code_checksum(&c)? != t.code_checksum {
                    Err(PauseReason::InvalidResponse)
                } else {
                    let metered = host.poll(&mut cache, &c, &request, f.gas_per_poll - gas)?;
                    gas = gas.saturating_add(metered.gas_used).min(f.gas_per_poll);
                    if metered.gas_used > f.gas_per_poll - self.config.host_gas {
                        Err(PauseReason::OutOfGas)
                    } else {
                        match metered.result {
                            Err(reason) => Err(reason),
                            Ok(response) => {
                                let mut staged = t.clone();
                                if self.stage(&mut cache, &mut staged, h, &response).is_err() {
                                    Err(PauseReason::InvalidResponse)
                                } else {
                                    let dispatched = host.dispatch(
                                        &mut cache,
                                        &c,
                                        &response.messages,
                                        f.gas_per_poll - gas,
                                    )?;
                                    let remaining = f.gas_per_poll - gas;
                                    gas =
                                        gas.saturating_add(dispatched.gas_used).min(f.gas_per_poll);
                                    if dispatched.gas_used > remaining {
                                        Err(PauseReason::OutOfGas)
                                    } else {
                                        dispatched.result.map(|()| response.outcome)
                                    }
                                }
                            }
                        }
                    }
                };
                if result.is_ok() {
                    cache.commit();
                    t = load(s, &c, t.task_id)?;
                    f = host::load(s, &addr, t.task_id)?;
                } else {
                    drop(cache);
                }
                let paid = self.keeper.attempt_charge_at(gas, base_price)?;
                f.available = f.available.checked_add(charge.checked_sub(paid)?)?;
                f.reserved = Uint128::zero();
                f.cumulative_execution_fees = f.cumulative_execution_fees.checked_add(paid)?;
                t.charged = t.charged.checked_add(paid)?;
                t.reserved = Uint128::zero();
                if !paid.is_zero() {
                    host.transfer(
                        s,
                        &self.keeper.escrow,
                        &self.config.fee_collector,
                        &Coin::new(paid.u128(), &self.keeper.config.denom),
                    )?;
                }
                match &result {
                    Err(reason) => {
                        t.status = ExecutionStatus::Paused;
                        f.status = TaskStatus::Paused {
                            reason: reason.clone(),
                            resume: if t.wait.is_some() && t.inbox.is_none() {
                                ResumeState::Waiting
                            } else {
                                ResumeState::Runnable
                            },
                        };
                    }
                    Ok(_) => {
                        f.status = if t.status == ExecutionStatus::Waiting {
                            TaskStatus::Waiting
                        } else {
                            TaskStatus::Runnable
                        };
                    }
                }
                host::save(s, &addr, &f)?;
                match result {
                    Ok(PollOutcome::Complete { .. }) => {
                        self.settle(s, &mut t, ExecutionStatus::Completed, host, &mut report)?
                    }
                    Ok(PollOutcome::Fail { .. }) => {
                        self.settle(s, &mut t, ExecutionStatus::Failed, host, &mut report)?
                    }
                    _ => {}
                }
                save(s, &t)?;
                report.polls += 1;
                report.gas_used += gas;
                // Poll and message execution share the task cap. Saturation
                // above bounds even an over-reporting host to that cap, and
                // reservation bounds the block sum without overflowing u64.
                report.poll_gas_used += gas - self.config.host_gas;
            }
        }
        report.next_base_price = next_base_price(
            base_price,
            report.poll_gas_used,
            self.config.block_gas / 2,
            self.keeper.config.gas_price,
            self.config.price_adjust_denominator,
        )?;
        put(s, b"na2/base-price", &report.next_base_price)?;
        put(s, b"na2/last-poll-gas", &report.poll_gas_used)?;
        put(s, b"na2/last-block", &h)?;
        Ok(report)
    }
}
