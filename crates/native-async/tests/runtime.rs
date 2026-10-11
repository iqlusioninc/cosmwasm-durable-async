#![cfg(feature = "host")]
use cosmwasm_std::{
    testing::{MockApi, MockStorage},
    Addr, Binary, Coin, CosmosMsg, Decimal, StdResult, Storage, Uint128,
};
use native_async::{host::Keeper, runtime::*, *};

struct TestHost {
    outcome: PollOutcome,
    fail: bool,
    dispatch_fail: bool,
    gas: u64,
}
impl PollHost for TestHost {
    fn code_checksum(&self, _: &str) -> StdResult<Binary> {
        Ok(Binary::from(b"code".as_slice()))
    }
    fn poll(
        &mut self,
        store: &mut dyn Storage,
        contract: &str,
        request: &PollRequest,
        _: u64,
    ) -> StdResult<Metered<PollResponse>> {
        store.set(b"user/write", b"committed");
        let mut log: Vec<(String, u64)> = store
            .get(b"user/log")
            .map(cosmwasm_std::from_json)
            .transpose()
            .unwrap()
            .unwrap_or_default();
        log.push((contract.to_string(), request.task_id));
        store.set(b"user/log", &cosmwasm_std::to_json_vec(&log).unwrap());
        store.set(
            b"user/request",
            &cosmwasm_std::to_json_vec(request).unwrap(),
        );
        Ok(Metered {
            gas_used: self.gas,
            result: if self.fail {
                Err(PauseReason::Trap)
            } else {
                Ok(PollResponse {
                    outcome: self.outcome.clone(),
                    messages: vec![],
                })
            },
        })
    }
    fn dispatch(
        &mut self,
        _: &mut dyn Storage,
        _: &str,
        _: &[CosmosMsg],
        _: u64,
    ) -> StdResult<Metered<()>> {
        Ok(Metered {
            gas_used: 0,
            result: if self.dispatch_fail {
                Err(PauseReason::MessageFailure)
            } else {
                Ok(())
            },
        })
    }
    fn transfer(
        &mut self,
        store: &mut dyn Storage,
        from: &Addr,
        to: &Addr,
        coin: &Coin,
    ) -> StdResult<()> {
        for (who, debit) in [(from, true), (to, false)] {
            let key = format!("bank/{who}");
            let amount: Uint128 = store
                .get(key.as_bytes())
                .map(cosmwasm_std::from_json)
                .transpose()?
                .unwrap_or_default();
            let amount = if debit {
                amount.checked_sub(coin.amount)?
            } else {
                amount.checked_add(coin.amount)?
            };
            store.set(key.as_bytes(), &cosmwasm_std::to_json_binary(&amount)?);
        }
        Ok(())
    }
}
fn setup() -> (Runtime, MockApi, MockStorage, Addr, TestHost) {
    let api = MockApi::default();
    let contract = api.addr_make("contract");
    let runtime = Runtime {
        keeper: Keeper {
            config: FundingConfig {
                denom: "ugas".into(),
                gas_price: Decimal::one(),
                max_gas_per_poll: 100,
                max_lifetime_blocks: 100,
            },
            escrow: api.addr_make("escrow"),
        },
        config: RuntimeConfig {
            block_gas: 120,
            host_gas: 2,
            maintenance_gas: 100,
            maintenance_gas_per_item: 1,
            max_visits: 10,
            max_promotions: 10,
            max_expirations: 10,
            max_prunes: 10,
            max_tasks: 10,
            max_contract_tasks: 10,
            max_allocated_bytes: 100_000,
            max_task_bytes: 10_000,
            max_messages: 10,
            max_message_bytes: 10_000,
            lease_price_per_byte_block: Uint128::zero(),
            cleanup_fee: Uint128::one(),
            retention_blocks: 2,
            fee_collector: api.addr_make("fees"),
        },
    };
    let mut storage = MockStorage::new();
    storage.set(
        format!("bank/{contract}").as_bytes(),
        &cosmwasm_std::to_json_binary(&Uint128::new(10000)).unwrap(),
    );
    let host = TestHost {
        outcome: PollOutcome::Complete {
            output: Binary::default(),
        },
        fail: false,
        dispatch_fail: false,
        gas: 7,
    };
    (runtime, api, storage, contract, host)
}
fn register(
    rt: &Runtime,
    api: &MockApi,
    store: &mut dyn Storage,
    c: &Addr,
    host: &mut TestHost,
) -> u64 {
    rt.register(
        api,
        store,
        1,
        c,
        SchedulerMsg::RegisterTask {
            control: c.to_string(),
            refund: c.to_string(),
            gas_per_poll: 40,
            max_gas_price: Decimal::one(),
            expires_at: 20,
            funds: Coin::new(1000u128, "ugas"),
        },
        TaskProgram {
            kind: "test".into(),
            version: 1,
            input: Binary::default(),
        },
        4096,
        host,
    )
    .unwrap()
}
#[test]
fn registration_waits_until_next_block_and_completion_refunds() {
    let (rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    assert_eq!(rt.run_block(&mut store, 1, &mut host).unwrap().polls, 0);
    assert_eq!(rt.run_block(&mut store, 2, &mut host).unwrap().polls, 1);
    assert_eq!(
        rt.task(&store, &c, id, 2).unwrap().status,
        ExecutionStatus::Completed
    );
    assert_eq!(store.get(b"user/write").unwrap(), b"committed");
    let funds: TaskFunding = cosmwasm_std::from_json(
        rt.keeper
            .query(
                &api,
                &store,
                2,
                SchedulerQuery::TaskFunding {
                    contract: c.to_string(),
                    task_id: id,
                },
            )
            .unwrap(),
    )
    .unwrap();
    assert!(funds.available.is_zero());
    assert_eq!(rt.task(&store, &c, id, 2).unwrap().charged, Uint128::new(9));
}
#[test]
fn trap_rolls_back_user_storage_but_charges_and_requires_retry() {
    let (rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    host.fail = true;
    rt.run_block(&mut store, 2, &mut host).unwrap();
    assert!(store.get(b"user/write").is_none());
    assert_eq!(rt.task(&store, &c, id, 2).unwrap().charged, Uint128::new(9));
    assert_eq!(rt.run_block(&mut store, 3, &mut host).unwrap().polls, 0);
}

#[test]
fn accepted_result_survives_deadline_and_restart_and_rejects_duplicates() {
    let (rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    host.outcome = PollOutcome::Pending {
        state: Binary::from(b"saved".as_slice()),
        wait: Wait::Operation {
            resolver: "service".into(),
            deadline: 5,
            max_result_bytes: 30,
        },
    };
    rt.run_block(&mut store, 2, &mut host).unwrap();
    let good = Resolution::Success {
        data: Binary::from(b"yes".as_slice()),
    };
    assert!(rt
        .deliver(&mut store, 3, &c, id, 1, "evil", good.clone(), false)
        .is_err());
    rt.deliver(&mut store, 4, &c, id, 1, "service", good.clone(), false)
        .unwrap();
    assert!(rt
        .deliver(&mut store, 4, &c, id, 1, "service", good.clone(), false)
        .is_err());
    assert_eq!(rt.task(&store, &c, id, 5).unwrap().inbox, Some(good));
    host.outcome = PollOutcome::Complete {
        output: Binary::from(b"done".as_slice()),
    };
    let restored = Runtime {
        keeper: rt.keeper,
        config: rt.config,
    };
    assert_eq!(
        restored.run_block(&mut store, 6, &mut host).unwrap().polls,
        1
    );
    assert_eq!(
        restored.task(&store, &c, id, 6).unwrap().status,
        ExecutionStatus::Completed
    );
}

#[test]
fn exact_deadline_and_expiry_hold_even_when_maintenance_is_disabled() {
    let (mut rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    host.outcome = PollOutcome::Pending {
        state: Binary::default(),
        wait: Wait::Operation {
            resolver: "service".into(),
            deadline: 5,
            max_result_bytes: 30,
        },
    };
    rt.run_block(&mut store, 2, &mut host).unwrap();
    rt.config.max_promotions = 0;
    rt.config.max_expirations = 0;
    assert!(rt
        .deliver(
            &mut store,
            5,
            &c,
            id,
            1,
            "service",
            Resolution::Success {
                data: Binary::default()
            },
            false
        )
        .is_err());
    assert_eq!(rt.run_block(&mut store, 20, &mut host).unwrap().polls, 0);
    assert_eq!(
        rt.task(&store, &c, id, 20).unwrap().status,
        ExecutionStatus::Expired
    );
    assert!(rt
        .manage(
            &api,
            &mut store,
            20,
            &c,
            SchedulerMsg::FundTask {
                task_id: id,
                contributor: c.to_string(),
                funds: Coin::new(1u128, "ugas")
            },
            &mut host
        )
        .is_err());
}

#[test]
fn cancellation_releases_migration_guard_and_prunes_without_reusing_ids() {
    let (rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    assert!(rt.assert_migration_allowed(&store, &c).is_err());
    rt.manage(
        &api,
        &mut store,
        2,
        &c,
        SchedulerMsg::CancelTask {
            task_id: id,
            actor: c.to_string(),
        },
        &mut host,
    )
    .unwrap();
    rt.assert_migration_allowed(&store, &c).unwrap();
    rt.run_block(&mut store, 22, &mut host).unwrap();
    assert!(rt.task(&store, &c, id, 22).is_err());
    let second = register(&rt, &api, &mut store, &c, &mut host);
    assert_eq!(second, 2);
}

#[test]
fn oversized_state_rolls_back_and_pauses_and_sponsor_does_not_retry() {
    let (rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    host.outcome = PollOutcome::Yield {
        state: Binary::from(vec![1; 4097]),
    };
    rt.run_block(&mut store, 2, &mut host).unwrap();
    assert!(store.get(b"user/write").is_none());
    rt.manage(
        &api,
        &mut store,
        3,
        &c,
        SchedulerMsg::FundTask {
            task_id: id,
            contributor: c.to_string(),
            funds: Coin::new(50u128, "ugas"),
        },
        &mut host,
    )
    .unwrap();
    assert_eq!(rt.run_block(&mut store, 3, &mut host).unwrap().polls, 0);
    host.outcome = PollOutcome::Complete {
        output: Binary::default(),
    };
    rt.manage(
        &api,
        &mut store,
        4,
        &c,
        SchedulerMsg::ResumeTask {
            task_id: id,
            actor: c.to_string(),
        },
        &mut host,
    )
    .unwrap();
    assert_eq!(rt.run_block(&mut store, 5, &mut host).unwrap().polls, 1);
}

#[test]
fn failed_message_and_out_of_gas_revert_user_state_and_bound_fees() {
    for out_of_gas in [false, true] {
        let (rt, api, mut store, c, mut host) = setup();
        let id = register(&rt, &api, &mut store, &c, &mut host);
        host.dispatch_fail = !out_of_gas;
        host.gas = if out_of_gas { u64::MAX } else { 7 };
        let report = rt.run_block(&mut store, 2, &mut host).unwrap();
        assert_eq!(report.gas_used, if out_of_gas { 40 } else { 9 });
        assert!(store.get(b"user/write").is_none());
        assert_eq!(
            rt.task(&store, &c, id, 2).unwrap().status,
            ExecutionStatus::Paused
        );
        assert_eq!(
            rt.task(&store, &c, id, 2).unwrap().charged,
            Uint128::new(if out_of_gas { 40 } else { 9 })
        );
    }
}

#[test]
fn round_robin_contracts_fifo_tasks_and_deferred_head_survive_restart() {
    let (mut rt, api, mut store, c, mut host) = setup();
    rt.config.block_gas = 60;
    let other = api.addr_make("other");
    store.set(
        format!("bank/{other}").as_bytes(),
        &cosmwasm_std::to_json_vec(&Uint128::new(10000)).unwrap(),
    );
    register(&rt, &api, &mut store, &c, &mut host);
    register(&rt, &api, &mut store, &c, &mut host);
    register(&rt, &api, &mut store, &other, &mut host);
    host.gas = 28;
    assert_eq!(rt.run_block(&mut store, 2, &mut host).unwrap().polls, 1);
    assert_eq!(rt.run_block(&mut store, 3, &mut host).unwrap().polls, 1);
    assert_eq!(rt.run_block(&mut store, 4, &mut host).unwrap().polls, 1);
    let log: Vec<(String, u64)> = cosmwasm_std::from_json(store.get(b"user/log").unwrap()).unwrap();
    assert_eq!(
        log,
        vec![
            (c.to_string(), 1),
            (other.to_string(), 1),
            (c.to_string(), 2)
        ]
    );
}

#[test]
fn yielding_once_per_block_and_timer_resume_resolution() {
    let (rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    host.outcome = PollOutcome::Yield {
        state: Binary::from(b"yield".as_slice()),
    };
    assert_eq!(rt.run_block(&mut store, 2, &mut host).unwrap().polls, 1);
    assert_eq!(rt.run_block(&mut store, 2, &mut host).unwrap().polls, 0);
    host.outcome = PollOutcome::Pending {
        state: Binary::from(b"timer".as_slice()),
        wait: Wait::Height { height: 6 },
    };
    rt.run_block(&mut store, 3, &mut host).unwrap();
    assert_eq!(rt.run_block(&mut store, 5, &mut host).unwrap().polls, 0);
    host.outcome = PollOutcome::Complete {
        output: Binary::default(),
    };
    rt.run_block(&mut store, 6, &mut host).unwrap();
    let request: PollRequest =
        cosmwasm_std::from_json(store.get(b"user/request").unwrap()).unwrap();
    assert_eq!(
        request.step,
        PollStep::Resume {
            state: Binary::from(b"timer".as_slice()),
            resolution: Resolution::Timer {}
        }
    );
    assert_eq!(
        rt.task(&store, &c, id, 6).unwrap().status,
        ExecutionStatus::Completed
    );
}

#[test]
fn storage_quota_accounts_for_metadata_and_rejects_registration_atomically() {
    let (rt, api, mut store, c, mut host) = setup();
    let before = store.get(format!("bank/{c}").as_bytes());
    let result = rt.register(
        &api,
        &mut store,
        1,
        &c,
        SchedulerMsg::RegisterTask {
            control: c.to_string(),
            refund: c.to_string(),
            gas_per_poll: 40,
            max_gas_price: Decimal::one(),
            expires_at: 20,
            funds: Coin::new(1000u128, "ugas"),
        },
        TaskProgram {
            kind: "test".into(),
            version: 1,
            input: Binary::default(),
        },
        1,
        &mut host,
    );
    assert!(result.is_err());
    assert_eq!(store.get(format!("bank/{c}").as_bytes()), before);
    assert_eq!(register(&rt, &api, &mut store, &c, &mut host), 1);
}

struct CallbackHost<'a> {
    inner: TestHost,
    runtime: &'a Runtime,
    contract: Addr,
    task_id: u64,
}
impl PollHost for CallbackHost<'_> {
    fn code_checksum(&self, c: &str) -> StdResult<Binary> {
        self.inner.code_checksum(c)
    }
    fn poll(
        &mut self,
        s: &mut dyn Storage,
        c: &str,
        r: &PollRequest,
        g: u64,
    ) -> StdResult<Metered<PollResponse>> {
        self.inner.poll(s, c, r, g)
    }
    fn dispatch(
        &mut self,
        s: &mut dyn Storage,
        c: &str,
        m: &[CosmosMsg],
        g: u64,
    ) -> StdResult<Metered<()>> {
        if matches!(self.inner.outcome, PollOutcome::Pending { .. }) {
            self.runtime
                .deliver(
                    s,
                    2,
                    &self.contract,
                    self.task_id,
                    1,
                    "service",
                    Resolution::Success {
                        data: Binary::from(b"immediate".as_slice()),
                    },
                    true,
                )
                .unwrap();
        }
        self.inner.dispatch(s, c, m, g)
    }
    fn transfer(&mut self, s: &mut dyn Storage, f: &Addr, t: &Addr, c: &Coin) -> StdResult<()> {
        self.inner.transfer(s, f, t, c)
    }
}
#[test]
fn immediate_callback_observes_staged_wait_but_only_runs_next_block() {
    let (rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    host.outcome = PollOutcome::Pending {
        state: Binary::from(b"saved".as_slice()),
        wait: Wait::Operation {
            resolver: "service".into(),
            deadline: 10,
            max_result_bytes: 20,
        },
    };
    let mut callback = CallbackHost {
        inner: host,
        runtime: &rt,
        contract: c.clone(),
        task_id: id,
    };
    assert_eq!(rt.run_block(&mut store, 2, &mut callback).unwrap().polls, 1);
    assert!(rt.task(&store, &c, id, 2).unwrap().inbox.is_some());
    assert_eq!(rt.run_block(&mut store, 2, &mut callback).unwrap().polls, 0);
    callback.inner.outcome = PollOutcome::Complete {
        output: Binary::default(),
    };
    assert_eq!(rt.run_block(&mut store, 3, &mut callback).unwrap().polls, 1);
    let request: PollRequest =
        cosmwasm_std::from_json(store.get(b"user/request").unwrap()).unwrap();
    assert_eq!(
        request.step,
        PollStep::Resume {
            state: Binary::from(b"saved".as_slice()),
            resolution: Resolution::Success {
                data: Binary::from(b"immediate".as_slice())
            }
        }
    );
}

#[test]
fn failed_outbound_discards_immediate_callback_and_retains_old_continuation() {
    let (rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    host.outcome = PollOutcome::Pending {
        state: Binary::from(b"saved".as_slice()),
        wait: Wait::Operation {
            resolver: "service".into(),
            deadline: 10,
            max_result_bytes: 20,
        },
    };
    host.dispatch_fail = true;
    let mut callback = CallbackHost {
        inner: host,
        runtime: &rt,
        contract: c.clone(),
        task_id: id,
    };
    rt.run_block(&mut store, 2, &mut callback).unwrap();
    let task = rt.task(&store, &c, id, 2).unwrap();
    assert_eq!(task.status, ExecutionStatus::Paused);
    assert!(task.inbox.is_none());
    assert!(task.state.is_none());
    assert_eq!(task.wait_sequence, 0);
    assert_eq!(task.charged, Uint128::new(9));
}

#[test]
fn application_failure_commits_and_refunds_but_expiry_never_invokes_guest() {
    let (rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    host.outcome = PollOutcome::Fail {
        error: "business failure".into(),
    };
    rt.run_block(&mut store, 2, &mut host).unwrap();
    let task = rt.task(&store, &c, id, 2).unwrap();
    assert_eq!(task.status, ExecutionStatus::Failed);
    assert!(store.get(b"user/write").is_some());
    let second = register(&rt, &api, &mut store, &c, &mut host);
    assert_eq!(rt.run_block(&mut store, 20, &mut host).unwrap().polls, 0);
    assert_eq!(
        rt.task(&store, &c, second, 20).unwrap().status,
        ExecutionStatus::Expired
    );
    rt.assert_migration_allowed(&store, &c).unwrap();
    let balance: Uint128 =
        cosmwasm_std::from_json(store.get(format!("bank/{c}").as_bytes()).unwrap()).unwrap();
    assert_eq!(balance, Uint128::new(9989)); // two cleanup fees and one 9-gas poll
}

#[test]
fn maintenance_and_promotion_caps_bound_work_without_scanning_sleepers() {
    let (mut rt, api, mut store, c, mut host) = setup();
    for _ in 0..5 {
        register(&rt, &api, &mut store, &c, &mut host);
    }
    rt.config.max_promotions = 1;
    let report = rt.run_block(&mut store, 2, &mut host).unwrap();
    assert_eq!(report.promotions, 1);
    assert_eq!(report.polls, 1);
    rt.config.maintenance_gas = 2;
    let report = rt.run_block(&mut store, 3, &mut host).unwrap();
    assert_eq!(report.maintenance_gas_used, 2);
    assert_eq!(report.polls, 0);
    assert_eq!(report.promotions, 0);
}

#[test]
fn lease_fees_are_prepaid_and_not_refunded_on_cancellation() {
    let (mut rt, api, mut store, c, mut host) = setup();
    rt.config.lease_price_per_byte_block = Uint128::one();
    store.set(
        format!("bank/{c}").as_bytes(),
        &cosmwasm_std::to_json_vec(&Uint128::new(200_000)).unwrap(),
    );
    let id = rt
        .register(
            &api,
            &mut store,
            1,
            &c,
            SchedulerMsg::RegisterTask {
                control: c.to_string(),
                refund: c.to_string(),
                gas_per_poll: 40,
                max_gas_price: Decimal::one(),
                expires_at: 20,
                funds: Coin::new(100_000u128, "ugas"),
            },
            TaskProgram {
                kind: "test".into(),
                version: 1,
                input: Binary::default(),
            },
            4096,
            &mut host,
        )
        .unwrap();
    assert_eq!(
        rt.task(&store, &c, id, 1).unwrap().storage_fee,
        Uint128::new(86017)
    ); // 4096 * (19 + 2) + 1
    rt.manage(
        &api,
        &mut store,
        2,
        &c,
        SchedulerMsg::CancelTask {
            task_id: id,
            actor: c.to_string(),
        },
        &mut host,
    )
    .unwrap();
    let balance: Uint128 =
        cosmwasm_std::from_json(store.get(format!("bank/{c}").as_bytes()).unwrap()).unwrap();
    assert_eq!(balance, Uint128::new(113983));
}

struct ReentrantHost<'a> {
    inner: TestHost,
    runtime: &'a Runtime,
    api: &'a MockApi,
    contract: Addr,
    task_id: u64,
    node_fault: bool,
}
impl PollHost for ReentrantHost<'_> {
    fn code_checksum(&self, c: &str) -> StdResult<Binary> {
        self.inner.code_checksum(c)
    }
    fn poll(
        &mut self,
        s: &mut dyn Storage,
        c: &str,
        r: &PollRequest,
        g: u64,
    ) -> StdResult<Metered<PollResponse>> {
        self.inner.poll(s, c, r, g)
    }
    fn dispatch(
        &mut self,
        s: &mut dyn Storage,
        _: &str,
        _: &[CosmosMsg],
        _: u64,
    ) -> StdResult<Metered<()>> {
        if self.node_fault {
            return Err(cosmwasm_std::StdError::generic_err("database fault"));
        }
        for msg in [
            SchedulerMsg::CancelTask {
                task_id: self.task_id,
                actor: self.contract.to_string(),
            },
            SchedulerMsg::ResumeTask {
                task_id: self.task_id,
                actor: self.contract.to_string(),
            },
        ] {
            assert!(self
                .runtime
                .manage(self.api, s, 2, &self.contract, msg, &mut self.inner)
                .is_err());
        }
        let funds: TaskFunding = cosmwasm_std::from_json(self.runtime.keeper.query(
            self.api,
            s,
            2,
            SchedulerQuery::TaskFunding {
                contract: self.contract.to_string(),
                task_id: self.task_id,
            },
        )?)
        .unwrap();
        assert_eq!(funds.reserved, Uint128::new(40));
        assert_eq!(funds.available, Uint128::new(959));
        self.runtime.manage(
            self.api,
            s,
            2,
            &self.contract,
            SchedulerMsg::FundTask {
                task_id: self.task_id,
                contributor: self.contract.to_string(),
                funds: Coin::new(50u128, "ugas"),
            },
            &mut self.inner,
        )?;
        Ok(Metered {
            gas_used: 0,
            result: Ok(()),
        })
    }
    fn transfer(&mut self, s: &mut dyn Storage, f: &Addr, t: &Addr, c: &Coin) -> StdResult<()> {
        self.inner.transfer(s, f, t, c)
    }
}

#[test]
fn reentrant_control_is_rejected_and_topup_survives_reservation_settlement() {
    let (rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    host.outcome = PollOutcome::Yield {
        state: Binary::default(),
    };
    let mut reentrant = ReentrantHost {
        inner: host,
        runtime: &rt,
        api: &api,
        contract: c.clone(),
        task_id: id,
        node_fault: false,
    };
    rt.run_block(&mut store, 2, &mut reentrant).unwrap();
    let funds: TaskFunding = cosmwasm_std::from_json(
        rt.keeper
            .query(
                &api,
                &store,
                2,
                SchedulerQuery::TaskFunding {
                    contract: c.to_string(),
                    task_id: id,
                },
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(funds.available, Uint128::new(1040)); // 1000 - cleanup 1 + topup 50 - actual 9
    assert_eq!(funds.reserved, Uint128::zero());
    assert_eq!(funds.cumulative_execution_fees, Uint128::new(9));
}

#[test]
fn node_fault_aborts_entire_block_without_charging_or_marking_height() {
    let (rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    host.outcome = PollOutcome::Yield {
        state: Binary::default(),
    };
    let mut reentrant = ReentrantHost {
        inner: host,
        runtime: &rt,
        api: &api,
        contract: c.clone(),
        task_id: id,
        node_fault: true,
    };
    assert!(rt.run_block(&mut store, 2, &mut reentrant).is_err());
    assert_eq!(rt.task(&store, &c, id, 2).unwrap().charged, Uint128::zero());
    assert!(store.get(b"user/write").is_none());
    reentrant.node_fault = false;
    assert_eq!(
        rt.run_block(&mut store, 2, &mut reentrant).unwrap().polls,
        1
    );
}

fn register_with_quota(
    rt: &Runtime,
    api: &MockApi,
    s: &mut dyn Storage,
    c: &Addr,
    host: &mut TestHost,
    quota: u64,
) -> StdResult<u64> {
    rt.register(
        api,
        s,
        1,
        c,
        SchedulerMsg::RegisterTask {
            control: c.to_string(),
            refund: c.to_string(),
            gas_per_poll: 40,
            max_gas_price: Decimal::one(),
            expires_at: 20,
            funds: Coin::new(1000u128, "ugas"),
        },
        TaskProgram {
            kind: "test".into(),
            version: 1,
            input: Binary::default(),
        },
        quota,
        host,
    )
}
fn minimum_quota() -> u64 {
    let (mut low, mut high) = (1, 4096);
    while low < high {
        let mid = (low + high) / 2;
        let (rt, api, mut s, c, mut host) = setup();
        if register_with_quota(&rt, &api, &mut s, &c, &mut host, mid).is_ok() {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    low
}
#[test]
fn exact_admission_boundary_reserves_maximum_topup_encoding() {
    let quota = minimum_quota();
    let (rt, api, mut s, c, mut host) = setup();
    assert!(register_with_quota(&rt, &api, &mut s, &c, &mut host, quota - 1).is_err());
    let id = register_with_quota(&rt, &api, &mut s, &c, &mut host, quota).unwrap();
    let before_size = runtime_records_size(&s, &c, id);
    s.set(
        format!("bank/{c}").as_bytes(),
        &cosmwasm_std::to_json_vec(&Uint128::MAX).unwrap(),
    );
    // Available and custody become u128::MAX; no numeric-width growth can
    // consume storage that was not reserved at admission.
    rt.manage(
        &api,
        &mut s,
        2,
        &c,
        SchedulerMsg::FundTask {
            task_id: id,
            contributor: c.to_string(),
            funds: Coin::new(Uint128::MAX.u128() - 999, "ugas"),
        },
        &mut host,
    )
    .unwrap();
    let after_size = runtime_records_size(&s, &c, id);
    assert!(after_size > before_size);
    assert!(
        after_size + 1024 <= quota as usize,
        "topup grew encoded records beyond the exact admitted allocation"
    );
}
fn runtime_records_size(s: &dyn Storage, c: &Addr, id: u64) -> usize {
    let task = [
        b"na2/task/".as_slice(),
        c.as_bytes(),
        b"/",
        &id.to_be_bytes(),
    ]
    .concat();
    let funding = [
        b"native-async/v1/task/".as_slice(),
        c.as_bytes(),
        b"/",
        &id.to_be_bytes(),
    ]
    .concat();
    s.get(&task).unwrap().len() + s.get(&funding).unwrap().len()
}
#[test]
fn cancellation_churn_removes_empty_ring_before_releasing_storage() {
    let (mut rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    // Promote without polling, leaving a contract ring entry to clean up.
    rt.config.max_visits = 0;
    rt.run_block(&mut s, 2, &mut host).unwrap();
    rt.manage(
        &api,
        &mut s,
        2,
        &c,
        SchedulerMsg::CancelTask {
            task_id: id,
            actor: c.to_string(),
        },
        &mut host,
    )
    .unwrap();
    rt.run_block(&mut s, 22, &mut host).unwrap();
    for prefix in [b"na2/ring/".as_slice(), b"na2/member/", b"na2/ready/"] {
        assert!(
            !s.range(None, None, cosmwasm_std::Order::Ascending)
                .any(|(k, _)| k.starts_with(prefix)),
            "released task left ring storage behind"
        );
    }
}

#[test]
fn exact_quota_covers_maximum_counters_and_every_pause_encoding() {
    let quota = minimum_quota();
    let (rt, api, mut s, c, mut host) = setup();
    let id = register_with_quota(&rt, &api, &mut s, &c, &mut host, quota).unwrap();
    let task_key = [
        b"na2/task/".as_slice(),
        c.as_bytes(),
        b"/",
        &id.to_be_bytes(),
    ]
    .concat();
    let funding_key = [
        b"native-async/v1/task/".as_slice(),
        c.as_bytes(),
        b"/",
        &id.to_be_bytes(),
    ]
    .concat();
    let mut task = rt.task(&s, &c, id, 1).unwrap();
    let mut funding: TaskFunding = cosmwasm_std::from_json(s.get(&funding_key).unwrap()).unwrap();
    // Model the maximum historical counters reachable through repeated funded
    // retries, without spending 2^128 units in the fixture.
    task.charged = Uint128::MAX.checked_sub(Uint128::new(9)).unwrap();
    task.wait_sequence = u64::MAX;
    task.last_attempt = Some(u64::MAX);
    funding.cumulative_execution_fees = task.charged;
    s.set(&task_key, &cosmwasm_std::to_json_vec(&task).unwrap());
    s.set(&funding_key, &cosmwasm_std::to_json_vec(&funding).unwrap());
    host.fail = true;
    assert_eq!(rt.run_block(&mut s, 2, &mut host).unwrap().polls, 1);
    let mut task = rt.task(&s, &c, id, 2).unwrap();
    assert_eq!(task.charged, Uint128::MAX);
    assert_eq!(task.status, ExecutionStatus::Paused);
    assert!(runtime_records_size(&s, &c, id) + 1024 <= quota as usize);
    let mut funding: TaskFunding = cosmwasm_std::from_json(s.get(&funding_key).unwrap()).unwrap();
    assert_eq!(funding.cumulative_execution_fees, Uint128::MAX);
    task.last_attempt = Some(u64::MAX);
    for reason in [
        PauseReason::InsufficientFunds,
        PauseReason::PriceLimit,
        PauseReason::OutOfGas,
        PauseReason::Trap,
        PauseReason::MessageFailure,
        PauseReason::InvalidResponse,
    ] {
        for resume in [ResumeState::Runnable, ResumeState::Waiting] {
            funding.status = TaskStatus::Paused {
                reason: reason.clone(),
                resume,
            };
            let bytes = cosmwasm_std::to_json_vec(&task).unwrap().len()
                + cosmwasm_std::to_json_vec(&funding).unwrap().len();
            assert!(
                bytes + 1024 <= quota as usize,
                "pause metadata exceeded prepaid capacity"
            );
        }
    }
}
