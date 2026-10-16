#![cfg(feature = "host")]
use cosmwasm_std::{
    testing::{MockApi, MockStorage},
    Addr, Binary, Coin, CosmosMsg, Decimal, StdResult, Storage, Uint128,
};
use native_async::{host::Keeper, runtime::*, *};

const TEST_RESOLVER: &str = "cosmwasm1qyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqsc7cyd3";

fn management(value: serde_json::Value) -> RuntimeMsg {
    serde_json::from_value(value).expect("management ABI")
}

#[allow(clippy::too_many_arguments)]
fn set_bid(
    rt: &Runtime,
    api: &MockApi,
    s: &mut dyn Storage,
    c: &Addr,
    host: &mut TestHost,
    id: u64,
    gas: u64,
    bid: &str,
) {
    set_bid_at(rt, api, s, c, host, 1, id, gas, bid);
}

#[allow(clippy::too_many_arguments)]
fn set_bid_at(
    rt: &Runtime,
    api: &MockApi,
    s: &mut dyn Storage,
    c: &Addr,
    host: &mut TestHost,
    height: u64,
    id: u64,
    gas: u64,
    bid: &str,
) {
    rt.execute(
        api,
        s,
        height,
        c,
        management(serde_json::json!({"update_budget": {
            "task_id": id, "actor": c, "gas_per_poll": gas, "max_gas_price": bid, "refund": c,
        }})),
        false,
        host,
    )
    .unwrap();
}

fn poll_log(s: &dyn Storage) -> Vec<(String, u64)> {
    s.get(b"user/log")
        .map(cosmwasm_std::from_json)
        .transpose()
        .unwrap()
        .unwrap_or_default()
}

fn set_base(s: &mut dyn Storage, price: &str) {
    s.set(
        b"na2/base-price",
        &cosmwasm_std::to_json_vec(&price.parse::<Decimal>().unwrap()).unwrap(),
    );
}

#[test]
fn priced_out_task_pauses_only_after_grace() {
    let (mut rt, api, mut s, c, mut host) = setup();
    rt.config.max_visits = 0;
    let id = register(&rt, &api, &mut s, &c, &mut host);
    for h in 2..=5 {
        set_base(&mut s, "2");
        let report = rt.run_block(&api, &mut s, h, &mut host).unwrap();
        assert_eq!(report.price_pauses, u32::from(h == 5));
        assert_eq!(
            rt.task(&s, &c, id, h).unwrap().status,
            if h == 5 {
                ExecutionStatus::Paused
            } else {
                ExecutionStatus::Runnable
            }
        );
    }
    assert_eq!(
        funding(&rt, &api, &s, &c, id).status,
        TaskStatus::Paused {
            reason: PauseReason::PriceLimit,
            resume: ResumeState::Runnable
        }
    );
    assert_eq!(
        s.range(
            Some(b"na2/bid/"),
            Some(b"na2/bid0"),
            cosmwasm_std::Order::Ascending
        )
        .count(),
        0
    );
}

#[test]
fn full_block_wait_never_price_pauses() {
    let (mut rt, api, mut s, c, mut host) = setup();
    rt.config.block_gas = 40;
    host.gas = 38;
    let high = register(&rt, &api, &mut s, &c, &mut host);
    set_bid(&rt, &api, &mut s, &c, &mut host, high, 40, "10");
    let id = register(&rt, &api, &mut s, &c, &mut host);
    set_bid(&rt, &api, &mut s, &c, &mut host, id, 40, "5");
    host.outcome = PollOutcome::Yield {
        state: Binary::default(),
    };
    for h in 2..=12 {
        let report = rt.run_block(&api, &mut s, h, &mut host).unwrap();
        assert_eq!(report.price_pauses, 0);
        assert_eq!(report.polls, 1);
        assert!(report.base_price <= "5".parse::<Decimal>().unwrap());
    }
    assert_eq!(
        rt.task(&s, &c, id, 12).unwrap().status,
        ExecutionStatus::Runnable
    );
    assert_eq!(poll_log(&s), vec![(c.to_string(), high); 11]);
}

#[test]
fn price_pause_survives_price_drop_until_resume_and_keeps_inbox() {
    let (mut rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    host.outcome = PollOutcome::Pending {
        state: Binary::from(b"saved".as_slice()),
        wait: Wait::Operation {
            resolver: TEST_RESOLVER.into(),
            deadline: 18,
            max_result_bytes: 20,
        },
    };
    rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    let resolution = Resolution::Success {
        data: Binary::from(b"reply".as_slice()),
    };
    rt.deliver(
        &mut s,
        3,
        &c,
        id,
        1,
        TEST_RESOLVER,
        resolution.clone(),
        false,
    )
    .unwrap();
    rt.config.max_visits = 0;
    for h in 3..=6 {
        set_base(&mut s, "2");
        rt.run_block(&api, &mut s, h, &mut host).unwrap();
    }
    assert_eq!(
        rt.task(&s, &c, id, 6).unwrap().status,
        ExecutionStatus::Paused
    );
    set_base(&mut s, "1");
    rt.run_block(&api, &mut s, 7, &mut host).unwrap();
    assert_eq!(
        rt.task(&s, &c, id, 7).unwrap().inbox,
        Some(resolution.clone())
    );
    assert_eq!(
        rt.task(&s, &c, id, 7).unwrap().status,
        ExecutionStatus::Paused
    );
    set_bid(&rt, &api, &mut s, &c, &mut host, id, 40, "2");
    rt.manage(
        &api,
        &mut s,
        7,
        &c,
        SchedulerMsg::ResumeTask {
            task_id: id,
            actor: c.to_string(),
        },
        &mut host,
    )
    .unwrap();
    assert_eq!(
        rt.task(&s, &c, id, 7).unwrap().status,
        ExecutionStatus::Runnable
    );
    assert_eq!(rt.task(&s, &c, id, 7).unwrap().inbox, Some(resolution));
}

#[test]
fn matured_height_timer_keeps_resolution_through_price_pause_and_resume() {
    matured_wait_keeps_resolution_through_price_pause_and_resume(
        Wait::Height { height: 4 },
        Resolution::Timer {},
    );
}

#[test]
fn matured_operation_timeout_keeps_resolution_through_price_pause_and_resume() {
    matured_wait_keeps_resolution_through_price_pause_and_resume(
        Wait::Operation {
            resolver: TEST_RESOLVER.into(),
            deadline: 4,
            max_result_bytes: 20,
        },
        Resolution::Timeout {},
    );
}

fn matured_wait_keeps_resolution_through_price_pause_and_resume(
    wait: Wait,
    resolution: Resolution,
) {
    let (rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    let state = Binary::from(b"saved".as_slice());
    host.outcome = PollOutcome::Pending {
        state: state.clone(),
        wait: wait.clone(),
    };
    assert_eq!(rt.run_block(&api, &mut s, 2, &mut host).unwrap().polls, 1);
    let due_key = s
        .range(
            Some(b"na2/due/"),
            Some(b"na2/due0"),
            cosmwasm_std::Order::Ascending,
        )
        .next()
        .unwrap()
        .0;
    let original_sequence = &due_key[due_key.len() - 8..];
    let query = |s: &dyn Storage, height| -> TaskResponse {
        cosmwasm_std::from_json(
            rt.query(
                &api,
                s,
                height,
                RuntimeQuery::Task {
                    contract: c.to_string(),
                    task_id: id,
                },
            )
            .unwrap(),
        )
        .unwrap()
    };
    assert_eq!(query(&s, 2).status, TaskStatus::Waiting);

    // Delayed promotion must retain the wait's original height and FIFO slot.
    for height in 5..=7 {
        set_base(&mut s, "2");
        let report = rt.run_block(&api, &mut s, height, &mut host).unwrap();
        assert_eq!(report.promotions, u32::from(height == 5));
        assert_eq!(report.polls, 0);
        assert_eq!(report.price_pauses, u32::from(height == 7));
        let public = query(&s, height);
        assert_eq!(public.inbox, Some(resolution.clone()));
        assert_eq!(public.wait, Some(wait.clone()));
        assert_eq!(public.state, Some(state.clone()));
        assert_eq!(public.last_attempt, Some(2));
        assert_eq!(rt.task(&s, &c, id, height).unwrap().ready_at, 4);
        assert_eq!(
            public.status,
            if height == 7 {
                TaskStatus::Paused {
                    reason: PauseReason::PriceLimit,
                    resume: ResumeState::Runnable,
                }
            } else {
                TaskStatus::Runnable
            }
        );
        if height < 7 {
            let bid_key = s
                .range(
                    Some(b"na2/bid/"),
                    Some(b"na2/bid0"),
                    cosmwasm_std::Order::Ascending,
                )
                .next()
                .unwrap()
                .0;
            assert_eq!(&bid_key[bid_key.len() - 8..], original_sequence);
        }
    }
    assert_eq!(
        rt.task(&s, &c, id, 7).unwrap().status,
        ExecutionStatus::Paused
    );
    set_base(&mut s, "1");
    rt.manage(
        &api,
        &mut s,
        7,
        &c,
        SchedulerMsg::ResumeTask {
            task_id: id,
            actor: c.to_string(),
        },
        &mut host,
    )
    .unwrap();
    let resumed = query(&s, 7);
    assert_eq!(resumed.status, TaskStatus::Runnable);
    assert_eq!(resumed.inbox, Some(resolution.clone()));
    assert_eq!(
        rt.task(&s, &c, id, 7).unwrap().status,
        ExecutionStatus::Runnable
    );
    host.outcome = PollOutcome::Complete {
        output: Binary::default(),
    };
    assert_eq!(rt.run_block(&api, &mut s, 8, &mut host).unwrap().polls, 1);
    let request: PollRequest = cosmwasm_std::from_json(s.get(b"user/request").unwrap()).unwrap();
    assert_eq!(request.step, PollStep::Resume { state, resolution });
    assert_eq!(query(&s, 8).status, TaskStatus::Completed);
}

#[test]
fn lowered_bid_grace_counts_from_last_poll() {
    let (mut rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    set_bid(&rt, &api, &mut s, &c, &mut host, id, 40, "3");
    host.outcome = PollOutcome::Yield {
        state: Binary::from(b"saved".as_slice()),
    };
    assert_eq!(rt.run_block(&api, &mut s, 2, &mut host).unwrap().polls, 1);
    assert_eq!(poll_log(&s), vec![(c.to_string(), id)]);
    assert_eq!(rt.task(&s, &c, id, 2).unwrap().last_attempt, Some(2));

    // Yield makes the task due at 3. Promote it, then leave it eligible but
    // unpolled until the later budget update at height 5.
    rt.config.max_visits = 0;
    for h in 3..=4 {
        set_base(&mut s, "2");
        assert_eq!(
            rt.run_block(&api, &mut s, h, &mut host)
                .unwrap()
                .price_pauses,
            0
        );
    }
    let before = rt.task(&s, &c, id, 4).unwrap();
    assert_eq!(before.ready_at, 3);
    assert_eq!(before.last_attempt, Some(2));
    set_bid_at(&rt, &api, &mut s, &c, &mut host, 5, id, 40, "1");
    let after = rt.task(&s, &c, id, 5).unwrap();
    assert_eq!(after.ready_at, 3);
    assert_eq!(after.last_attempt, Some(2));
    for h in 5..=6 {
        set_base(&mut s, "2");
        let report = rt.run_block(&api, &mut s, h, &mut host).unwrap();
        assert_eq!(report.price_pauses, u32::from(h == 6));
    }
    assert_eq!(
        rt.task(&s, &c, id, 6).unwrap().status,
        ExecutionStatus::Paused
    );
    assert_eq!(poll_log(&s), vec![(c.to_string(), id)]);
}

#[test]
fn registration_rejects_bid_below_floor_admits_below_base() {
    let (rt, api, mut s, c, mut host) = setup();
    set_base(&mut s, "2");
    let bad = rt.register(
        &api,
        &mut s,
        1,
        &c,
        SchedulerMsg::RegisterTask {
            control: c.to_string(),
            refund: c.to_string(),
            gas_per_poll: 40,
            max_gas_price: "0.5".parse().unwrap(),
            expires_at: 20,
            funds: Coin::new(1000u128, "ugas"),
        },
        TaskProgram {
            kind: "test".into(),
            version: 1,
            input: Binary::default(),
        },
        4096,
        &mut host,
    );
    assert!(bad.is_err());
    let id = register(&rt, &api, &mut s, &c, &mut host);
    assert_eq!(id, 1);
    assert_eq!(funding(&rt, &api, &s, &c, id).status, TaskStatus::Runnable);
    let stored: TaskFunding = cosmwasm_std::from_json(
        s.get(
            &[
                b"native-async/v1/task/".as_slice(),
                c.as_bytes(),
                b"/",
                &id.to_be_bytes(),
            ]
            .concat(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(stored.max_attempt_charge, Uint128::new(80));
}

#[test]
fn price_walk_cap_counts_younger_skipped_entries() {
    let (mut rt, api, mut s, c, mut host) = setup();
    rt.config.max_visits = 0;
    rt.config.max_price_pauses = 1;
    let old = register(&rt, &api, &mut s, &c, &mut host);
    set_bid(&rt, &api, &mut s, &c, &mut host, old, 40, "2");
    set_base(&mut s, "4");
    rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    let young = rt
        .register(
            &api,
            &mut s,
            4,
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
            4096,
            &mut host,
        )
        .unwrap();
    set_base(&mut s, "4");
    let report = rt.run_block(&api, &mut s, 5, &mut host).unwrap();
    assert_eq!(report.price_pauses, 0);
    assert_eq!(
        rt.task(&s, &c, old, 5).unwrap().status,
        ExecutionStatus::Runnable
    );
    assert_eq!(
        rt.task(&s, &c, young, 5).unwrap().status,
        ExecutionStatus::Runnable
    );
    rt.config.max_price_pauses = 2;
    set_base(&mut s, "4");
    assert_eq!(
        rt.run_block(&api, &mut s, 6, &mut host)
            .unwrap()
            .price_pauses,
        1
    );
    assert_eq!(
        rt.task(&s, &c, old, 6).unwrap().status,
        ExecutionStatus::Paused
    );
}

#[test]
fn price_walk_obeys_shared_maintenance_gas() {
    let (mut rt, api, mut s, c, mut host) = setup();
    rt.config.max_visits = 0;
    let id = register(&rt, &api, &mut s, &c, &mut host);
    set_base(&mut s, "2");
    rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    rt.config.maintenance_gas = 3;
    set_base(&mut s, "2");
    let report = rt.run_block(&api, &mut s, 5, &mut host).unwrap();
    assert_eq!(report.maintenance_gas_used, 3);
    assert_eq!(report.price_pauses, 0);
    assert_eq!(
        rt.task(&s, &c, id, 5).unwrap().status,
        ExecutionStatus::Runnable
    );
    rt.config.maintenance_gas = 4;
    set_base(&mut s, "2");
    assert_eq!(
        rt.run_block(&api, &mut s, 6, &mut host)
            .unwrap()
            .price_pauses,
        1
    );
}

#[test]
fn expiry_precedes_price_pause_even_when_expiry_maintenance_is_exhausted() {
    let (mut rt, api, mut s, c, mut host) = setup();
    rt.config.max_visits = 0;
    rt.config.max_expirations = 0;
    let id = rt
        .register(
            &api,
            &mut s,
            1,
            &c,
            SchedulerMsg::RegisterTask {
                control: c.to_string(),
                refund: c.to_string(),
                gas_per_poll: 40,
                max_gas_price: Decimal::one(),
                expires_at: 5,
                funds: Coin::new(1000u128, "ugas"),
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
    set_base(&mut s, "2");
    rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    set_base(&mut s, "2");
    assert_eq!(
        rt.run_block(&api, &mut s, 5, &mut host)
            .unwrap()
            .price_pauses,
        0
    );
    assert_eq!(
        rt.task(&s, &c, id, 5).unwrap().status,
        ExecutionStatus::Expired
    );
    rt.config.max_expirations = 1;
    assert_eq!(
        rt.run_block(&api, &mut s, 6, &mut host)
            .unwrap()
            .expirations,
        1
    );
}

#[test]
fn resume_requires_bid_at_or_above_base_price() {
    let (mut rt, api, mut s, c, mut host) = setup();
    rt.config.max_visits = 0;
    let id = register(&rt, &api, &mut s, &c, &mut host);
    for h in 2..=5 {
        set_base(&mut s, "2");
        rt.run_block(&api, &mut s, h, &mut host).unwrap();
    }
    assert!(rt
        .manage(
            &api,
            &mut s,
            5,
            &c,
            SchedulerMsg::ResumeTask {
                task_id: id,
                actor: c.to_string()
            },
            &mut host
        )
        .is_err());
    set_bid(&rt, &api, &mut s, &c, &mut host, id, 40, "2");
    rt.manage(
        &api,
        &mut s,
        5,
        &c,
        SchedulerMsg::ResumeTask {
            task_id: id,
            actor: c.to_string(),
        },
        &mut host,
    )
    .unwrap();
    assert_eq!(
        rt.task(&s, &c, id, 5).unwrap().status,
        ExecutionStatus::Runnable
    );
}

#[test]
fn highest_bid_polls_first_and_equal_bids_are_fifo() {
    let (mut rt, api, mut s, c, mut host) = setup();
    rt.config.block_gas = 40;
    host.gas = 38;
    for bid in ["3", "5", "5"] {
        let id = register(&rt, &api, &mut s, &c, &mut host);
        set_bid(&rt, &api, &mut s, &c, &mut host, id, 40, bid);
    }
    for h in 2..=4 {
        assert_eq!(rt.run_block(&api, &mut s, h, &mut host).unwrap().polls, 1);
    }
    assert_eq!(
        poll_log(&s),
        vec![(c.to_string(), 2), (c.to_string(), 3), (c.to_string(), 1)]
    );
}

#[test]
fn stops_at_first_bid_below_base_price() {
    let (rt, api, mut s, c, mut host) = setup();
    for bid in ["3", "5", "2"] {
        let id = register(&rt, &api, &mut s, &c, &mut host);
        set_bid(&rt, &api, &mut s, &c, &mut host, id, 40, bid);
    }
    s.set(
        b"na2/base-price",
        &cosmwasm_std::to_json_vec(&Decimal::percent(400)).unwrap(),
    );
    let report = rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    assert_eq!(poll_log(&s), vec![(c.to_string(), 2)]);
    assert!(report.visits <= 2);
    assert_eq!(
        rt.task(&s, &c, 1, 2).unwrap().status,
        ExecutionStatus::Runnable
    );
    assert_eq!(
        s.range(
            Some(b"na2/bid/"),
            Some(b"na2/bid0"),
            cosmwasm_std::Order::Ascending
        )
        .count(),
        2
    );
}

#[test]
fn bid_equal_to_base_price_is_eligible() {
    let (rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    set_bid(&rt, &api, &mut s, &c, &mut host, id, 40, "4");
    s.set(
        b"na2/base-price",
        &cosmwasm_std::to_json_vec(&Decimal::percent(400)).unwrap(),
    );
    assert_eq!(rt.run_block(&api, &mut s, 2, &mut host).unwrap().polls, 1);
}

#[test]
fn all_priced_out_polls_nothing_and_decays() {
    let (rt, api, mut s, c, mut host) = setup();
    for _ in 0..3 {
        register(&rt, &api, &mut s, &c, &mut host);
    }
    s.set(
        b"na2/base-price",
        &cosmwasm_std::to_json_vec(&Decimal::percent(400)).unwrap(),
    );
    let report = rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    assert_eq!(report.polls, 0);
    assert_eq!(report.visits, 1);
    assert!(report.next_base_price < report.base_price);
    assert_eq!(
        rt.task(&s, &c, 1, 2).unwrap().status,
        ExecutionStatus::Runnable
    );
}

#[test]
fn oversized_task_is_skipped_then_first_next_block() {
    let (mut rt, api, mut s, c, mut host) = setup();
    rt.config.block_gas = 100;
    host.gas = 38;
    for (bid, gas) in [("5", 40), ("4", 80), ("3", 40)] {
        let id = register(&rt, &api, &mut s, &c, &mut host);
        set_bid(&rt, &api, &mut s, &c, &mut host, id, gas, bid);
    }
    let report = rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    assert_eq!(report.polls, 2);
    assert_eq!(report.visits, 3);
    assert_eq!(poll_log(&s), vec![(c.to_string(), 1), (c.to_string(), 3)]);
    assert_eq!(rt.run_block(&api, &mut s, 3, &mut host).unwrap().polls, 1);
    assert_eq!(poll_log(&s).last(), Some(&(c.to_string(), 2)));
}

#[test]
fn selection_visit_cap_is_global_across_oversized_entries() {
    let (mut rt, api, mut s, c, mut host) = setup();
    rt.config.block_gas = 100;
    rt.config.max_visits = 3;
    host.gas = 38;
    for (bid, gas) in [("5", 40), ("4", 80), ("3", 80), ("2", 40)] {
        let id = register(&rt, &api, &mut s, &c, &mut host);
        set_bid(&rt, &api, &mut s, &c, &mut host, id, gas, bid);
    }
    let report = rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    assert_eq!(report.visits, 3);
    assert_eq!(report.polls, 1);
    assert_eq!(
        rt.task(&s, &c, 4, 2).unwrap().status,
        ExecutionStatus::Runnable
    );
}

#[test]
fn update_budget_rekeys_bid_entry_preserving_fifo() {
    let (mut rt, api, mut s, c, mut host) = setup();
    for bid in ["3", "5", "5"] {
        let id = register(&rt, &api, &mut s, &c, &mut host);
        set_bid(&rt, &api, &mut s, &c, &mut host, id, 40, bid);
    }
    rt.config.max_visits = 0;
    rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    // Raising the oldest task to an equal bid must retain its FIFO position.
    set_bid(&rt, &api, &mut s, &c, &mut host, 1, 40, "5");
    set_bid(&rt, &api, &mut s, &c, &mut host, 3, 40, "6");
    rt.config.max_visits = 1;
    for h in 3..=5 {
        assert_eq!(rt.run_block(&api, &mut s, h, &mut host).unwrap().polls, 1);
    }
    assert_eq!(
        poll_log(&s),
        vec![(c.to_string(), 3), (c.to_string(), 1), (c.to_string(), 2)]
    );
}

#[test]
fn delayed_promotion_preserves_original_readiness_height() {
    let (mut rt, api, mut s, c, mut host) = setup();
    for _ in 0..2 {
        register(&rt, &api, &mut s, &c, &mut host);
    }
    rt.config.max_promotions = 1;
    rt.config.max_visits = 0;
    rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    rt.run_block(&api, &mut s, 5, &mut host).unwrap();
    for id in 1..=2 {
        let task = serde_json::to_value(rt.task(&s, &c, id, 5).unwrap()).unwrap();
        assert_eq!(task["ready_at"], 2);
    }
    set_bid(&rt, &api, &mut s, &c, &mut host, 2, 40, "2");
    assert_eq!(
        serde_json::to_value(rt.task(&s, &c, 2, 5).unwrap()).unwrap()["ready_at"],
        2
    );
}

#[test]
fn delayed_promotion_keeps_fifo_before_equal_height_delivery() {
    let (mut rt, api, mut s, c, mut host) = setup();
    let waiting = register(&rt, &api, &mut s, &c, &mut host);
    let delayed = register(&rt, &api, &mut s, &c, &mut host);
    rt.config.max_promotions = 1;
    rt.config.max_visits = 1;
    host.outcome = PollOutcome::Pending {
        state: Binary::from(b"wait".as_slice()),
        wait: Wait::Operation {
            resolver: TEST_RESOLVER.into(),
            deadline: 10,
            max_result_bytes: 10,
        },
    };
    assert_eq!(rt.run_block(&api, &mut s, 2, &mut host).unwrap().polls, 1);
    rt.deliver(
        &mut s,
        2,
        &c,
        waiting,
        1,
        TEST_RESOLVER,
        Resolution::Success {
            data: Binary::default(),
        },
        false,
    )
    .unwrap();
    host.outcome = PollOutcome::Complete {
        output: Binary::default(),
    };
    assert_eq!(rt.run_block(&api, &mut s, 3, &mut host).unwrap().polls, 1);
    assert_eq!(
        poll_log(&s),
        vec![(c.to_string(), waiting), (c.to_string(), delayed)]
    );
}

#[test]
fn base_price_rises_on_full_blocks_and_decays_to_floor() {
    let (rt, api, mut s, c, mut host) = setup();
    for _ in 0..3 {
        register(&rt, &api, &mut s, &c, &mut host);
    }
    host.gas = 35;
    let report = rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    assert_eq!(report.polls, 3);
    assert_eq!(report.base_price, Decimal::one());
    assert_eq!(report.poll_gas_used, 105);
    assert_eq!(
        report.next_base_price,
        Decimal::from_ratio(175u128, 160u128)
    );
    assert!(report.next_base_price > report.base_price);
    assert_eq!(rt.base_price(&s).unwrap(), report.next_base_price);
    let idle = rt.run_block(&api, &mut s, 3, &mut host).unwrap();
    assert_eq!(idle.base_price, report.next_base_price);
    assert_eq!(idle.poll_gas_used, 0);
    assert_eq!(idle.next_base_price, Decimal::one());
    assert_eq!(rt.base_price(&s).unwrap(), Decimal::one());
    assert_eq!(s.get(b"na2/last-poll-gas").unwrap(), b"0");
}

#[test]
fn polls_are_charged_at_block_base_price() {
    let (rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    rt.execute(
        &api,
        &mut s,
        1,
        &c,
        management(serde_json::json!({"update_budget": {
            "task_id": id, "actor": c, "gas_per_poll": 40,
            "max_gas_price": "3", "refund": c,
        }})),
        false,
        &mut host,
    )
    .unwrap();
    s.set(
        b"na2/base-price",
        &cosmwasm_std::to_json_vec(&Decimal::percent(200)).unwrap(),
    );
    let report = rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    assert_eq!(report.polls, 1);
    assert_eq!(report.base_price, Decimal::percent(200));
    assert_eq!(report.gas_used, 9);
    assert_eq!(report.poll_gas_used, 7); // Fixed host accounting is billed, not utilization.
    assert_eq!(rt.task(&s, &c, id, 2).unwrap().charged, Uint128::new(18));
    assert_eq!(s.get(b"na2/last-poll-gas").unwrap(), b"7");
    let funding: TaskFunding = cosmwasm_std::from_json(
        rt.query(
            &api,
            &s,
            2,
            RuntimeQuery::Funding(SchedulerQuery::TaskFunding {
                contract: c.to_string(),
                task_id: id,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(funding.max_attempt_charge, Uint128::new(72)); // ceil(40 * 1.779166666666666667)
}

// A task admitted with a representable reservation can become unaffordable
// after a legal price increase. Overflow must not abort unrelated block work.
#[test]
fn reservation_overflow_after_price_increase_pauses_without_charge() {
    let (mut rt, api, mut s, c, mut host) = setup();
    let cap = 2_000_000_000_000_000_000u64;
    rt.config.block_gas = cap;
    rt.config.host_gas = 70_000_000;
    rt.keeper.config.max_gas_per_poll = cap;
    set_base(&mut s, "170000000000000000000");
    s.set(
        format!("bank/{c}").as_bytes(),
        &cosmwasm_std::to_json_vec(&Uint128::MAX).unwrap(),
    );
    let registration = |gas, bid: &str, funds: u128| SchedulerMsg::RegisterTask {
        control: c.to_string(),
        refund: c.to_string(),
        gas_per_poll: gas,
        max_gas_price: bid.parse().unwrap(),
        expires_at: 20,
        funds: Coin::new(funds, "ugas"),
    };
    let program = TaskProgram {
        kind: "test".into(),
        version: 1,
        input: Binary::default(),
    };
    let id = rt
        .register(
            &api,
            &mut s,
            1,
            &c,
            registration(
                cap,
                "200000000000000000000",
                340000000000000000000000000000000000001,
            ),
            program.clone(),
            4096,
            &mut host,
        )
        .unwrap();
    let initial: TaskFunding = cosmwasm_std::from_json(
        rt.query(
            &api,
            &s,
            1,
            RuntimeQuery::Funding(SchedulerQuery::TaskFunding {
                contract: c.to_string(),
                task_id: id,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        initial.max_attempt_charge,
        Uint128::new(340000000000000000000000000000000000000)
    );
    let small = rt
        .register(
            &api,
            &mut s,
            1,
            &c,
            registration(
                80_000_000,
                "199000000000000000000",
                20000000000000000000000000001,
            ),
            program.clone(),
            4096,
            &mut host,
        )
        .unwrap();
    host.gas = cap - rt.config.host_gas;
    host.outcome = PollOutcome::Pending {
        state: Binary::from(b"saved".as_slice()),
        wait: Wait::Operation {
            resolver: TEST_RESOLVER.into(),
            deadline: 10,
            max_result_bytes: 20,
        },
    };
    let full = rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    assert_eq!(full.polls, 1);
    assert_eq!(full.poll_gas_used, 1_999_999_999_930_000_000);
    assert_eq!(
        full.next_base_price,
        "191249999998512500000".parse::<Decimal>().unwrap()
    );
    rt.deliver(
        &mut s,
        2,
        &c,
        id,
        1,
        TEST_RESOLVER,
        Resolution::Success {
            data: Binary::from(b"yes".as_slice()),
        },
        false,
    )
    .unwrap();
    let before = rt.task(&s, &c, id, 2).unwrap();
    assert_eq!(
        before.charged,
        Uint128::new(340000000000000000000000000000000000000)
    );
    assert!(before.inbox.is_some());
    let query = RuntimeQuery::Funding(SchedulerQuery::TaskFunding {
        contract: c.to_string(),
        task_id: id,
    });
    assert!(
        rt.query(&api, &s, 2, query).is_err(),
        "quote must remain Uint128"
    );
    let snapshot: Vec<_> = s
        .range(None, None, cosmwasm_std::Order::Ascending)
        .collect();
    assert!(rt
        .register(
            &api,
            &mut s,
            2,
            &c,
            registration(cap, "200000000000000000000", Uint128::MAX.u128()),
            program,
            4096,
            &mut host
        )
        .is_err());
    let budget = management(serde_json::json!({"update_budget":{
        "task_id":id,"actor":c,"refund":c,"gas_per_poll":cap,
        "max_gas_price":"200000000000000000000",
    }}));
    assert!(rt
        .execute(&api, &mut s, 2, &c, budget, false, &mut host)
        .is_err());
    assert_eq!(
        snapshot,
        s.range(None, None, cosmwasm_std::Order::Ascending)
            .collect::<Vec<_>>()
    );
    host.gas = 7;
    let report = rt.run_block(&api, &mut s, 3, &mut host).unwrap();
    assert_eq!(
        report.polls, 1,
        "another affordable task continues in the same block"
    );
    assert_eq!(
        poll_log(&s),
        vec![(c.to_string(), id), (c.to_string(), small)]
    );
    let after = rt.task(&s, &c, id, 3).unwrap();
    assert_eq!(after.status, ExecutionStatus::Paused);
    assert_eq!(after.charged, before.charged);
    assert_eq!(after.state, before.state);
    assert_eq!(after.inbox, before.inbox);
    assert_eq!(after.wait, before.wait);
    assert_eq!(after.last_attempt, before.last_attempt);
    let key = [
        b"native-async/v1/task/".as_slice(),
        c.as_bytes(),
        b"/",
        &id.to_be_bytes(),
    ]
    .concat();
    let funds: TaskFunding = cosmwasm_std::from_json(s.get(&key).unwrap()).unwrap();
    assert_eq!(
        funds.status,
        TaskStatus::Paused {
            reason: PauseReason::InsufficientFunds,
            resume: ResumeState::Runnable
        }
    );
    assert!(funds.available.is_zero());
    // Reapply the observed congestion price to check the paused task's resume
    // rejects an impossible quote, rather than only its exhausted balance.
    set_base(&mut s, "191249999998512500000");
    let saved = cosmwasm_std::to_json_vec(&after).unwrap();
    assert!(rt
        .manage(
            &api,
            &mut s,
            3,
            &c,
            SchedulerMsg::ResumeTask {
                task_id: id,
                actor: c.to_string()
            },
            &mut host
        )
        .is_err());
    assert_eq!(
        saved,
        cosmwasm_std::to_json_vec(&rt.task(&s, &c, id, 3).unwrap()).unwrap()
    );
}

#[test]
fn funding_config_reports_live_price() {
    let (rt, api, mut s, _, mut host) = setup();
    assert_eq!(rt.base_price(&s).unwrap(), Decimal::one());
    s.set(
        b"na2/base-price",
        &cosmwasm_std::to_json_vec(&Decimal::percent(200)).unwrap(),
    );
    s.set(b"na2/last-poll-gas", b"17");
    let config: FundingConfig = cosmwasm_std::from_json(
        rt.query(
            &api,
            &s,
            2,
            RuntimeQuery::Funding(SchedulerQuery::FundingConfig {}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(config.gas_price, Decimal::percent(200));
    assert_eq!(config.min_base_price, Decimal::one());
    assert_eq!(config.target_gas, 60);
    assert_eq!(config.price_grace_blocks, 3);
    assert_eq!(config.last_block_poll_gas_used, 17);
    let report = rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    assert_eq!(report.next_base_price, Decimal::percent(175));
    let replay = rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    assert_eq!(replay.polls, 0);
    assert_eq!(rt.base_price(&s).unwrap(), Decimal::percent(175));
}

#[test]
fn runtime_rejects_zero_price_target_and_denominator_without_writes() {
    for (block_gas, denominator) in [(1, 8), (120, 0)] {
        let (mut rt, api, mut s, _, mut host) = setup();
        rt.config.block_gas = block_gas;
        rt.config.price_adjust_denominator = denominator;
        assert!(rt.run_block(&api, &mut s, 2, &mut host).is_err());
        assert!(s.get(b"na2/base-price").is_none());
        assert!(s.get(b"na2/last-block").is_none());
    }
}

#[test]
fn registration_and_budget_update_reserve_at_live_price() {
    let (rt, api, mut s, c, mut host) = setup();
    s.set(
        b"na2/base-price",
        &cosmwasm_std::to_json_vec(&Decimal::percent(200)).unwrap(),
    );
    let id = register(&rt, &api, &mut s, &c, &mut host);
    // A bid below the live price is admitted for later eligibility/grace handling.
    let registered = funding(&rt, &api, &s, &c, id);
    assert_eq!(registered.status, TaskStatus::Runnable);
    let stored: TaskFunding = cosmwasm_std::from_json(
        s.get(
            &[
                b"native-async/v1/task/".as_slice(),
                c.as_bytes(),
                b"/",
                &id.to_be_bytes(),
            ]
            .concat(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(stored.max_attempt_charge, Uint128::new(80));
    rt.execute(
        &api,
        &mut s,
        1,
        &c,
        management(serde_json::json!({"update_budget": {
            "task_id": id, "actor": c, "gas_per_poll": 60,
            "max_gas_price": "3", "refund": c,
        }})),
        false,
        &mut host,
    )
    .unwrap();
    let stored: TaskFunding = cosmwasm_std::from_json(
        s.get(
            &[
                b"native-async/v1/task/".as_slice(),
                c.as_bytes(),
                b"/",
                &id.to_be_bytes(),
            ]
            .concat(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(stored.max_attempt_charge, Uint128::new(120));
}

#[test]
fn resume_checks_current_base_price() {
    let (rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    host.gas = 45;
    rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    s.set(
        b"na2/base-price",
        &cosmwasm_std::to_json_vec(&Decimal::percent(200)).unwrap(),
    );
    assert!(rt
        .manage(
            &api,
            &mut s,
            3,
            &c,
            SchedulerMsg::ResumeTask {
                task_id: id,
                actor: c.to_string(),
            },
            &mut host
        )
        .is_err());
}

#[test]
fn overreported_poll_gas_is_capped_before_price_utilization() {
    let (rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    host.gas = u64::MAX;
    let report = rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    assert_eq!(report.gas_used, 40);
    assert_eq!(report.poll_gas_used, 38);
    assert_eq!(rt.task(&s, &c, id, 2).unwrap().charged, Uint128::new(40));
    assert_eq!(report.next_base_price, Decimal::one());
}

#[test]
fn zero_floor_runtime_registers_and_charges_zero() {
    let (mut rt, api, mut s, c, mut host) = setup();
    rt.keeper.config.gas_price = Decimal::zero();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    let report = rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    assert_eq!(report.polls, 1);
    assert_eq!(report.base_price, Decimal::zero());
    assert_eq!(report.next_base_price, Decimal::zero());
    assert_eq!(report.poll_gas_used, 7);
    assert_eq!(rt.task(&s, &c, id, 2).unwrap().charged, Uint128::zero());
}

#[test]
fn zero_floor_runtime_queries_live_funding() {
    let (mut rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    rt.keeper.config.gas_price = Decimal::zero();
    let config: FundingConfig = cosmwasm_std::from_json(
        rt.query(
            &api,
            &s,
            1,
            RuntimeQuery::Funding(SchedulerQuery::FundingConfig {}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(config.gas_price, Decimal::zero());
    assert_eq!(config.min_base_price, Decimal::zero());
    let task: TaskFunding = cosmwasm_std::from_json(
        rt.query(
            &api,
            &s,
            1,
            RuntimeQuery::Funding(SchedulerQuery::TaskFunding {
                contract: c.to_string(),
                task_id: id,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(task.max_attempt_charge, Uint128::zero());
    let task: TaskResponse = cosmwasm_std::from_json(
        rt.query(
            &api,
            &s,
            1,
            RuntimeQuery::Task {
                contract: c.to_string(),
                task_id: id,
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(task.status, TaskStatus::Runnable);
}

#[test]
fn zero_floor_runtime_funds_task() {
    let (mut rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    rt.keeper.config.gas_price = Decimal::zero();
    rt.manage(
        &api,
        &mut s,
        1,
        &c,
        SchedulerMsg::FundTask {
            task_id: id,
            contributor: c.to_string(),
            funds: Coin::new(7u128, "ugas"),
        },
        &mut host,
    )
    .unwrap();
    assert_eq!(bank(&s, &rt.keeper.escrow), Uint128::new(1006));
}

#[test]
fn zero_floor_runtime_resumes_task() {
    let (mut rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    host.gas = 45;
    rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    rt.keeper.config.gas_price = Decimal::zero();
    s.set(
        b"na2/base-price",
        &cosmwasm_std::to_json_vec(&Decimal::zero()).unwrap(),
    );
    rt.manage(
        &api,
        &mut s,
        3,
        &c,
        SchedulerMsg::ResumeTask {
            task_id: id,
            actor: c.to_string(),
        },
        &mut host,
    )
    .unwrap();
    assert_eq!(
        rt.task(&s, &c, id, 3).unwrap().status,
        ExecutionStatus::Runnable
    );
}

#[test]
fn zero_floor_runtime_cancels_task() {
    let (mut rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    rt.keeper.config.gas_price = Decimal::zero();
    rt.manage(
        &api,
        &mut s,
        1,
        &c,
        SchedulerMsg::CancelTask {
            task_id: id,
            actor: c.to_string(),
        },
        &mut host,
    )
    .unwrap();
    assert_eq!(
        rt.task(&s, &c, id, 1).unwrap().status,
        ExecutionStatus::Cancelled
    );
    assert_eq!(bank(&s, &rt.keeper.escrow), Uint128::zero());
}

#[test]
fn registration_requires_lease_cleanup_and_one_live_price_poll_atomically() {
    let (mut rt, api, mut s, c, mut host) = setup();
    rt.config.lease_price_per_byte_block = Uint128::one();
    s.set(
        format!("bank/{c}").as_bytes(),
        &cosmwasm_std::to_json_vec(&Uint128::new(100_000)).unwrap(),
    );
    s.set(
        b"na2/base-price",
        &cosmwasm_std::to_json_vec(&Decimal::percent(200)).unwrap(),
    );
    // Rent: 4096 * (19 + 2), cleanup: 1, live-price poll: 40 * 2.
    for amount in [86057u128, 86096, 86097] {
        let before = snapshot(&s);
        let result = rt.register(
            &api,
            &mut s,
            1,
            &c,
            SchedulerMsg::RegisterTask {
                control: c.to_string(),
                refund: c.to_string(),
                gas_per_poll: 40,
                max_gas_price: Decimal::one(),
                expires_at: 20,
                funds: Coin::new(amount, "ugas"),
            },
            TaskProgram {
                kind: "test".into(),
                version: 1,
                input: Binary::default(),
            },
            4096,
            &mut host,
        );
        if amount < 86097 {
            assert!(result.is_err(), "accepted underfunded amount {amount}");
            assert_eq!(
                snapshot(&s),
                before,
                "bank, task, counters, and indexes must roll back"
            );
        } else {
            assert_eq!(result.unwrap(), 1);
            assert_eq!(
                rt.task(&s, &c, 1, 1).unwrap().status,
                ExecutionStatus::Runnable
            );
            assert_eq!(bank(&s, &rt.keeper.escrow), Uint128::new(80));
            assert_eq!(bank(&s, &rt.config.fee_collector), Uint128::new(86017));
        }
    }
}

#[test]
fn public_task_query_projects_lifecycle_without_internal_fields() {
    let (rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    let query = |s: &dyn Storage, height| -> serde_json::Value {
        cosmwasm_std::from_json(
            rt.query(
                &api,
                s,
                height,
                RuntimeQuery::Task {
                    contract: c.to_string(),
                    task_id: id,
                },
            )
            .unwrap(),
        )
        .unwrap()
    };
    let initial = query(&s, 1);
    assert_eq!(initial.as_object().unwrap().len(), 15);
    for private in [
        "queue_key",
        "expiry_key",
        "storage_fee",
        "charged",
        "reserved",
    ] {
        assert!(initial.get(private).is_none(), "exposed {private}");
    }
    assert_eq!(initial["last_attempt"], serde_json::Value::Null);
    assert_eq!(initial["state"], serde_json::Value::Null);
    host.outcome = PollOutcome::Pending {
        state: Binary::from(b"continuation"),
        wait: Wait::Height { height: 4 },
    };
    rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    assert_eq!(query(&s, 2)["status"], "waiting");
    assert_eq!(query(&s, 2)["last_attempt"], 2);
    assert_eq!(query(&s, 20)["status"], "expired");
    host.outcome = PollOutcome::Complete {
        output: Binary::from(b"done"),
    };
    rt.run_block(&api, &mut s, 4, &mut host).unwrap();
    let completed = query(&s, 4);
    assert_eq!(completed["status"], "completed");
    assert_eq!(completed["output"], "ZG9uZQ==");
    assert_eq!(completed["error"], serde_json::Value::Null);
}

#[test]
fn public_task_query_preserves_typed_pause_and_failed_output_separation() {
    for fails in [false, true] {
        let (rt, api, mut s, c, mut host) = setup();
        let id = register(&rt, &api, &mut s, &c, &mut host);
        if fails {
            host.outcome = PollOutcome::Fail {
                error: "program failed".into(),
            };
        } else {
            host.gas = 45;
        }
        rt.run_block(&api, &mut s, 2, &mut host).unwrap();
        let response: TaskResponse = cosmwasm_std::from_json(
            rt.query(
                &api,
                &s,
                2,
                RuntimeQuery::Task {
                    contract: c.to_string(),
                    task_id: id,
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(response.last_attempt, Some(2));
        assert_eq!(response.output, None);
        if fails {
            assert_eq!(response.status, TaskStatus::Failed);
            assert_eq!(response.error.as_deref(), Some("program failed"));
        } else {
            assert_eq!(
                response.status,
                TaskStatus::Paused {
                    reason: PauseReason::OutOfGas,
                    resume: ResumeState::Runnable
                }
            );
            assert_eq!(response.error, None);
        }
    }
}
fn funding(rt: &Runtime, api: &MockApi, s: &dyn Storage, c: &Addr, id: u64) -> TaskFunding {
    cosmwasm_std::from_json(
        rt.query(
            api,
            s,
            3,
            RuntimeQuery::Funding(SchedulerQuery::TaskFunding {
                contract: c.to_string(),
                task_id: id,
            }),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn extend_lease_charges_exact_incremental_rent_and_preserves_wait() {
    for (bytes, expiry, rent) in [(4196, 20, 1900), (4096, 21, 4096), (4196, 21, 6096)] {
        let (mut rt, api, mut s, c, mut host) = setup();
        let id = register(&rt, &api, &mut s, &c, &mut host);
        host.outcome = PollOutcome::Pending {
            state: Binary::default(),
            wait: Wait::Operation {
                resolver: TEST_RESOLVER.into(),
                deadline: 5,
                max_result_bytes: 10,
            },
        };
        rt.run_block(&api, &mut s, 2, &mut host).unwrap();
        rt.manage(
            &api,
            &mut s,
            3,
            &c,
            SchedulerMsg::FundTask {
                task_id: id,
                contributor: c.to_string(),
                funds: Coin::new(8000u128, "ugas"),
            },
            &mut host,
        )
        .unwrap();
        rt.config.lease_price_per_byte_block = Uint128::one();
        let before = funding(&rt, &api, &s, &c, id);
        let old = rt.task(&s, &c, id, 3).unwrap();
        rt.execute(
            &api,
            &mut s,
            3,
            &c,
            management(serde_json::json!({"extend_lease": {
                "task_id": id, "actor": c, "allocated_bytes": bytes, "expires_at": expiry,
            }})),
            false,
            &mut host,
        )
        .unwrap();
        let after = funding(&rt, &api, &s, &c, id);
        assert_eq!(before.available - after.available, Uint128::new(rent));
        assert_eq!(after.storage_lease_paid, Uint128::new(rent + 1));
        let updated = rt.task(&s, &c, id, 3).unwrap();
        assert_eq!(updated.wait, old.wait);
        assert_eq!(updated.wait_sequence, old.wait_sequence);
        assert_eq!(updated.inbox, old.inbox);
        assert_eq!(updated.status, old.status);
        assert_eq!(updated.expires_at, expiry);
        assert_eq!(updated.retain_until, expiry + 2);
        assert_eq!(updated.allocated_bytes, bytes);
    }
}

#[test]
fn budget_update_keeps_pause_until_explicit_retry_and_changes_refund() {
    let (rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    host.gas = 45;
    rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    let refund = api.addr_make("new-refund");
    rt.execute(
        &api,
        &mut s,
        3,
        &c,
        management(serde_json::json!({"update_budget": {
            "task_id": id, "actor": c, "gas_per_poll": 60, "max_gas_price": "1.5", "refund": refund,
        }})),
        false,
        &mut host,
    )
    .unwrap();
    let f = funding(&rt, &api, &s, &c, id);
    assert_eq!(f.max_attempt_charge, Uint128::new(60));
    assert_eq!(f.refund, refund.as_str());
    assert_eq!(rt.run_block(&api, &mut s, 3, &mut host).unwrap().polls, 0);
    rt.manage(
        &api,
        &mut s,
        3,
        &c,
        SchedulerMsg::ResumeTask {
            task_id: id,
            actor: c.to_string(),
        },
        &mut host,
    )
    .unwrap();
    assert_eq!(rt.run_block(&api, &mut s, 4, &mut host).unwrap().polls, 1);
    assert_eq!(
        rt.task(&s, &c, id, 4).unwrap().status,
        ExecutionStatus::Completed
    );
    let balance: Uint128 =
        cosmwasm_std::from_json(s.get(format!("bank/{refund}").as_bytes()).unwrap()).unwrap();
    assert_eq!(balance, Uint128::new(912));
}

fn snapshot(s: &dyn Storage) -> Vec<(Vec<u8>, Vec<u8>)> {
    s.range(None, None, cosmwasm_std::Order::Ascending)
        .collect()
}

#[test]
fn policy_validation_and_insufficient_rent_are_fully_atomic() {
    let (mut rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    let sponsor = api.addr_make("sponsor");
    rt.manage(
        &api,
        &mut s,
        2,
        &c,
        SchedulerMsg::FundTask {
            task_id: id,
            contributor: sponsor.to_string(),
            funds: Coin::new(1u128, "ugas"),
        },
        &mut host,
    )
    .unwrap();
    let extend = |actor: &Addr, bytes, expiry| {
        management(serde_json::json!({"extend_lease":{
            "task_id":id,"actor":actor,"allocated_bytes":bytes,"expires_at":expiry,
        }}))
    };
    let budget = |actor: &Addr, gas, refund: &str| {
        management(serde_json::json!({"update_budget":{
            "task_id":id,"actor":actor,"gas_per_poll":gas,"max_gas_price":"1","refund":refund,
        }}))
    };
    for (h, msg) in [
        (3, extend(&sponsor, 5000, 25)),
        (3, budget(&sponsor, 60, c.as_str())),
        (20, extend(&c, 5000, 25)),
        (20, budget(&c, 60, c.as_str())),
        (3, extend(&c, 4096, 20)),
        (3, extend(&c, 4000, 21)),
        (3, extend(&c, 5000, 19)),
        (3, extend(&c, 10001, 21)),
        (3, extend(&c, 5000, 104)),
        (3, extend(&c, 5000, u64::MAX)),
        (3, budget(&c, 0, c.as_str())),
        (3, budget(&c, 2, c.as_str())),
        (3, budget(&c, 101, c.as_str())),
        (3, budget(&c, 60, "invalid")),
        (3, budget(&c, 60, rt.keeper.escrow.as_str())),
    ] {
        let before = snapshot(&s);
        assert!(rt
            .execute(&api, &mut s, h, &c, msg, false, &mut host)
            .is_err());
        assert_eq!(snapshot(&s), before);
    }
    rt.config.max_allocated_bytes = 4096;
    let before = snapshot(&s);
    assert!(rt
        .execute(&api, &mut s, 3, &c, extend(&c, 4097, 20), false, &mut host)
        .is_err());
    assert_eq!(snapshot(&s), before);
    rt.config.max_allocated_bytes = 100_000;
    rt.config.lease_price_per_byte_block = Uint128::one();
    assert!(rt
        .execute(&api, &mut s, 3, &c, extend(&c, 5000, 25), false, &mut host)
        .is_err());
    assert_eq!(snapshot(&s), before);
    rt.config.lease_price_per_byte_block = Uint128::MAX;
    assert!(rt
        .execute(&api, &mut s, 3, &c, extend(&c, 5000, 25), false, &mut host)
        .is_err());
    assert_eq!(snapshot(&s), before);
    rt.config.retention_blocks = u64::MAX;
    assert!(rt
        .execute(&api, &mut s, 3, &c, extend(&c, 5000, 25), false, &mut host)
        .is_err());
    assert_eq!(snapshot(&s), before);
}

#[test]
fn extended_expiry_index_and_retention_survive_restart() {
    let (rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    host.outcome = PollOutcome::Pending {
        state: Binary::default(),
        wait: Wait::Height { height: 19 },
    };
    rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    rt.execute(
        &api,
        &mut s,
        3,
        &c,
        management(serde_json::json!({"extend_lease":{
            "task_id":id,"actor":c,"allocated_bytes":5000,"expires_at":25,
        }})),
        false,
        &mut host,
    )
    .unwrap();
    let expiry_records: Vec<_> = snapshot(&s)
        .into_iter()
        .filter(|(k, _)| k.starts_with(b"na2/expiry/"))
        .collect();
    assert_eq!(expiry_records.len(), 1);
    assert_eq!(
        &expiry_records[0].0[b"na2/expiry/".len()..b"na2/expiry/".len() + 8],
        &25u64.to_be_bytes()
    );
    let mut restored = Runtime {
        keeper: rt.keeper,
        config: rt.config,
    };
    restored.config.block_gas = 2; // Positive price target, without invoking guest.
    assert_eq!(
        restored
            .run_block(&api, &mut s, 20, &mut host)
            .unwrap()
            .expirations,
        0
    );
    assert_eq!(
        restored
            .run_block(&api, &mut s, 25, &mut host)
            .unwrap()
            .expirations,
        1
    );
    assert_eq!(
        restored
            .run_block(&api, &mut s, 26, &mut host)
            .unwrap()
            .prunes,
        0
    );
    assert_eq!(
        restored
            .run_block(&api, &mut s, 27, &mut host)
            .unwrap()
            .prunes,
        1
    );
    assert!(restored.task(&s, &c, id, 27).is_err());
}

#[test]
fn budget_noop_is_accepted_price_limit_blocks_resume_and_cancel_uses_new_refund() {
    let (rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    host.fail = true;
    rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    let refund = api.addr_make("new-refund");
    let msg = management(serde_json::json!({"update_budget":{
        "task_id":id,"actor":c,"gas_per_poll":60,"max_gas_price":"0.5","refund":refund,
    }}));
    rt.execute(&api, &mut s, 3, &c, msg.clone(), false, &mut host)
        .unwrap();
    let before = snapshot(&s);
    rt.execute(&api, &mut s, 3, &c, msg, false, &mut host)
        .unwrap();
    assert_eq!(snapshot(&s), before);
    assert!(rt
        .manage(
            &api,
            &mut s,
            3,
            &c,
            SchedulerMsg::ResumeTask {
                task_id: id,
                actor: c.to_string()
            },
            &mut host
        )
        .is_err());
    rt.manage(
        &api,
        &mut s,
        3,
        &c,
        SchedulerMsg::CancelTask {
            task_id: id,
            actor: c.to_string(),
        },
        &mut host,
    )
    .unwrap();
    let balance: Uint128 =
        cosmwasm_std::from_json(s.get(format!("bank/{refund}").as_bytes()).unwrap()).unwrap();
    assert_eq!(balance, Uint128::new(990));
}

#[test]
fn budget_can_exceed_balance_but_resume_requires_explicit_topup() {
    let (mut rt, api, mut s, c, mut host) = setup();
    rt.config.cleanup_fee = Uint128::new(960);
    let id = register(&rt, &api, &mut s, &c, &mut host);
    let msg = management(serde_json::json!({"update_budget":{
        "task_id":id,"actor":c,"gas_per_poll":60,"max_gas_price":"1","refund":c,
    }}));
    rt.execute(&api, &mut s, 3, &c, msg, false, &mut host)
        .unwrap();
    // Admission funded the original cap exactly; increasing the cap can
    // still cause an insufficient-funds pause when the scheduler visits it.
    assert_eq!(rt.run_block(&api, &mut s, 3, &mut host).unwrap().polls, 0);
    let before = funding(&rt, &api, &s, &c, id);
    assert_eq!(before.available, Uint128::new(40));
    assert_eq!(before.storage_lease_paid, Uint128::new(960));
    assert_eq!(before.cumulative_execution_fees, Uint128::zero());
    assert!(rt
        .manage(
            &api,
            &mut s,
            3,
            &c,
            SchedulerMsg::ResumeTask {
                task_id: id,
                actor: c.to_string()
            },
            &mut host
        )
        .is_err());
    rt.manage(
        &api,
        &mut s,
        3,
        &c,
        SchedulerMsg::FundTask {
            task_id: id,
            contributor: api.addr_make("sponsor").to_string(),
            funds: Coin::new(20u128, "ugas"),
        },
        &mut host,
    )
    .unwrap();
    assert_eq!(rt.run_block(&api, &mut s, 3, &mut host).unwrap().polls, 0);
    rt.manage(
        &api,
        &mut s,
        3,
        &c,
        SchedulerMsg::ResumeTask {
            task_id: id,
            actor: c.to_string(),
        },
        &mut host,
    )
    .unwrap();
    assert_eq!(rt.run_block(&api, &mut s, 4, &mut host).unwrap().polls, 1);
}

#[test]
fn budget_update_does_not_bypass_an_operation_wait_or_discard_accepted_inbox() {
    let (rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    host.outcome = PollOutcome::Pending {
        state: Binary::default(),
        wait: Wait::Operation {
            resolver: TEST_RESOLVER.into(),
            deadline: 10,
            max_result_bytes: 20,
        },
    };
    rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    let budget = management(serde_json::json!({"update_budget":{
        "task_id":id,"actor":c,"gas_per_poll":60,"max_gas_price":"1","refund":c,
    }}));
    rt.execute(&api, &mut s, 3, &c, budget.clone(), false, &mut host)
        .unwrap();
    assert_eq!(rt.run_block(&api, &mut s, 3, &mut host).unwrap().polls, 0);
    rt.deliver(
        &mut s,
        4,
        &c,
        id,
        1,
        TEST_RESOLVER,
        Resolution::Success {
            data: Binary::from(b"yes".as_slice()),
        },
        false,
    )
    .unwrap();
    let before = cosmwasm_std::to_json_binary(&rt.task(&s, &c, id, 4).unwrap()).unwrap();
    rt.execute(&api, &mut s, 4, &c, budget, false, &mut host)
        .unwrap();
    assert_eq!(
        cosmwasm_std::to_json_binary(&rt.task(&s, &c, id, 4).unwrap()).unwrap(),
        before
    );
}

struct TestHost {
    outcome: PollOutcome,
    fail: bool,
    dispatch_fail: bool,
    gas: u64,
    transfers: u32,
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
        self.transfers += 1;
        if store.get(format!("bank/blocked/{to}").as_bytes()).is_some() {
            return Err(cosmwasm_std::StdError::generic_err("send restricted"));
        }
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
                min_base_price: Decimal::one(),
                target_gas: 60,
                price_grace_blocks: 3,
                last_block_poll_gas_used: 0,
                max_gas_per_poll: 100,
                max_lifetime_blocks: 100,
            },
            escrow: api.addr_make("escrow"),
        },
        config: RuntimeConfig {
            block_gas: 120,
            price_grace_blocks: 3,
            price_adjust_denominator: 8,
            max_price_pauses: 10,
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
        transfers: 0,
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
    assert_eq!(
        rt.run_block(&api, &mut store, 1, &mut host).unwrap().polls,
        0
    );
    assert_eq!(
        rt.run_block(&api, &mut store, 2, &mut host).unwrap().polls,
        1
    );
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
    rt.run_block(&api, &mut store, 2, &mut host).unwrap();
    assert!(store.get(b"user/write").is_none());
    assert_eq!(rt.task(&store, &c, id, 2).unwrap().charged, Uint128::new(9));
    assert_eq!(
        rt.run_block(&api, &mut store, 3, &mut host).unwrap().polls,
        0
    );
}

#[test]
fn accepted_result_survives_deadline_and_restart_and_rejects_duplicates() {
    let (rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    host.outcome = PollOutcome::Pending {
        state: Binary::from(b"saved".as_slice()),
        wait: Wait::Operation {
            resolver: TEST_RESOLVER.into(),
            deadline: 5,
            max_result_bytes: 30,
        },
    };
    rt.run_block(&api, &mut store, 2, &mut host).unwrap();
    let good = Resolution::Success {
        data: Binary::from(b"yes".as_slice()),
    };
    assert!(rt
        .deliver(&mut store, 3, &c, id, 1, "evil", good.clone(), false)
        .is_err());
    rt.deliver(&mut store, 4, &c, id, 1, TEST_RESOLVER, good.clone(), false)
        .unwrap();
    assert!(rt
        .deliver(&mut store, 4, &c, id, 1, TEST_RESOLVER, good.clone(), false)
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
        restored
            .run_block(&api, &mut store, 6, &mut host)
            .unwrap()
            .polls,
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
            resolver: TEST_RESOLVER.into(),
            deadline: 5,
            max_result_bytes: 30,
        },
    };
    rt.run_block(&api, &mut store, 2, &mut host).unwrap();
    rt.config.max_promotions = 0;
    rt.config.max_expirations = 0;
    assert!(rt
        .deliver(
            &mut store,
            5,
            &c,
            id,
            1,
            TEST_RESOLVER,
            Resolution::Success {
                data: Binary::default()
            },
            false
        )
        .is_err());
    assert_eq!(
        rt.run_block(&api, &mut store, 20, &mut host).unwrap().polls,
        0
    );
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
    rt.run_block(&api, &mut store, 22, &mut host).unwrap();
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
    rt.run_block(&api, &mut store, 2, &mut host).unwrap();
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
    assert_eq!(
        rt.run_block(&api, &mut store, 3, &mut host).unwrap().polls,
        0
    );
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
    assert_eq!(
        rt.run_block(&api, &mut store, 5, &mut host).unwrap().polls,
        1
    );
}

#[test]
fn failed_message_and_out_of_gas_revert_user_state_and_bound_fees() {
    for out_of_gas in [false, true] {
        let (rt, api, mut store, c, mut host) = setup();
        let id = register(&rt, &api, &mut store, &c, &mut host);
        host.dispatch_fail = !out_of_gas;
        host.gas = if out_of_gas { u64::MAX } else { 7 };
        let report = rt.run_block(&api, &mut store, 2, &mut host).unwrap();
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
fn equal_bid_contracts_share_global_fifo_and_deferred_entries_survive_restart() {
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
    assert_eq!(
        rt.run_block(&api, &mut store, 2, &mut host).unwrap().polls,
        1
    );
    let (mut restored, _, _, _, _) = setup();
    restored.config.block_gas = 60;
    assert_eq!(
        restored
            .run_block(&api, &mut store, 3, &mut host)
            .unwrap()
            .polls,
        1
    );
    assert_eq!(
        restored
            .run_block(&api, &mut store, 4, &mut host)
            .unwrap()
            .polls,
        1
    );
    let log: Vec<(String, u64)> = cosmwasm_std::from_json(store.get(b"user/log").unwrap()).unwrap();
    assert_eq!(
        log,
        vec![
            (c.to_string(), 1),
            (c.to_string(), 2),
            (other.to_string(), 1)
        ]
    );
    assert!(!store
        .range(None, None, cosmwasm_std::Order::Ascending)
        .any(|(k, _)| k.starts_with(b"na2/ring")
            || k.starts_with(b"na2/member/")
            || k.starts_with(b"na2/ready/")));
}

#[test]
fn yielding_once_per_block_and_timer_resume_resolution() {
    let (rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    host.outcome = PollOutcome::Yield {
        state: Binary::from(b"yield".as_slice()),
    };
    assert_eq!(
        rt.run_block(&api, &mut store, 2, &mut host).unwrap().polls,
        1
    );
    assert_eq!(
        rt.run_block(&api, &mut store, 2, &mut host).unwrap().polls,
        0
    );
    host.outcome = PollOutcome::Pending {
        state: Binary::from(b"timer".as_slice()),
        wait: Wait::Height { height: 6 },
    };
    rt.run_block(&api, &mut store, 3, &mut host).unwrap();
    assert_eq!(
        rt.run_block(&api, &mut store, 5, &mut host).unwrap().polls,
        0
    );
    host.outcome = PollOutcome::Complete {
        output: Binary::default(),
    };
    rt.run_block(&api, &mut store, 6, &mut host).unwrap();
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
                    TEST_RESOLVER,
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
    for zero_price in [false, true] {
        let (mut rt, api, mut store, c, mut host) = setup();
        if zero_price {
            rt.keeper.config.gas_price = Decimal::zero();
            rt.keeper.config.min_base_price = Decimal::zero();
        }
        let id = register(&rt, &api, &mut store, &c, &mut host);
        host.outcome = PollOutcome::Pending {
            state: Binary::from(b"saved".as_slice()),
            wait: Wait::Operation {
                resolver: TEST_RESOLVER.into(),
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
        assert_eq!(
            rt.run_block(&api, &mut store, 2, &mut callback)
                .unwrap()
                .polls,
            1
        );
        assert_no_flight_markers(&store);
        assert!(rt.task(&store, &c, id, 2).unwrap().inbox.is_some());
        assert_eq!(
            rt.run_block(&api, &mut store, 2, &mut callback)
                .unwrap()
                .polls,
            0
        );
        callback.inner.outcome = PollOutcome::Complete {
            output: Binary::default(),
        };
        assert_eq!(
            rt.run_block(&api, &mut store, 3, &mut callback)
                .unwrap()
                .polls,
            1
        );
        assert_no_flight_markers(&store);
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
}

#[test]
fn failed_outbound_discards_immediate_callback_and_retains_old_continuation() {
    let (rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    host.outcome = PollOutcome::Pending {
        state: Binary::from(b"saved".as_slice()),
        wait: Wait::Operation {
            resolver: TEST_RESOLVER.into(),
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
    rt.run_block(&api, &mut store, 2, &mut callback).unwrap();
    assert_no_flight_markers(&store);
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
    rt.run_block(&api, &mut store, 2, &mut host).unwrap();
    let task = rt.task(&store, &c, id, 2).unwrap();
    assert_eq!(task.status, ExecutionStatus::Failed);
    assert!(store.get(b"user/write").is_some());
    let second = register(&rt, &api, &mut store, &c, &mut host);
    assert_eq!(
        rt.run_block(&api, &mut store, 20, &mut host).unwrap().polls,
        0
    );
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
    let report = rt.run_block(&api, &mut store, 2, &mut host).unwrap();
    assert_eq!(report.promotions, 1);
    assert_eq!(report.polls, 1);
    rt.config.maintenance_gas = 2;
    let report = rt.run_block(&api, &mut store, 3, &mut host).unwrap();
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
    other_task: Option<u64>,
    check_during_poll: bool,
    deposit: Uint128,
    dispatch_gas: u64,
}
impl ReentrantHost<'_> {
    fn check_controls(&mut self, s: &mut dyn Storage) {
        let before: Vec<_> = s
            .range(None, None, cosmwasm_std::Order::Ascending)
            .collect();
        let same_task_cancel = self.runtime.manage(
            self.api,
            s,
            2,
            &self.contract,
            SchedulerMsg::CancelTask {
                task_id: self.task_id,
                actor: self.contract.to_string(),
            },
            &mut self.inner,
        );
        assert!(same_task_cancel.is_err());
        let same_task_resume = self.runtime.manage(
            self.api,
            s,
            2,
            &self.contract,
            SchedulerMsg::ResumeTask {
                task_id: self.task_id,
                actor: self.contract.to_string(),
            },
            &mut self.inner,
        );
        assert!(same_task_resume.is_err());
        let same_task_extend = self.runtime.execute(
            self.api,
            s,
            2,
            &self.contract,
            RuntimeMsg::ExtendLease {
                task_id: self.task_id,
                actor: self.contract.to_string(),
                allocated_bytes: 5000,
                expires_at: 25,
            },
            true,
            &mut self.inner,
        );
        assert!(same_task_extend.is_err());
        let same_task_budget = self.runtime.execute(
            self.api,
            s,
            2,
            &self.contract,
            RuntimeMsg::UpdateBudget {
                task_id: self.task_id,
                actor: self.contract.to_string(),
                gas_per_poll: 60,
                max_gas_price: Decimal::one(),
                refund: self.contract.to_string(),
            },
            true,
            &mut self.inner,
        );
        assert!(same_task_budget.is_err());
        for error in [
            same_task_cancel.err(),
            same_task_resume.err(),
            same_task_extend.err(),
            same_task_budget.err(),
        ] {
            assert!(error.unwrap().to_string().contains("in-flight"));
        }
        assert_eq!(
            before,
            s.range(None, None, cosmwasm_std::Order::Ascending)
                .collect::<Vec<_>>()
        );
        if let Some(task_id) = self.other_task {
            let authorized_other_task_management = self.runtime.execute(
                self.api,
                s,
                2,
                &self.contract,
                RuntimeMsg::UpdateBudget {
                    task_id,
                    actor: self.contract.to_string(),
                    gas_per_poll: 40,
                    max_gas_price: Decimal::one(),
                    refund: self.contract.to_string(),
                },
                true,
                &mut self.inner,
            );
            assert!(authorized_other_task_management.is_ok());
            assert_eq!(
                before,
                s.range(None, None, cosmwasm_std::Order::Ascending)
                    .collect::<Vec<_>>()
            );
        }
    }
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
        if self.check_during_poll {
            self.check_controls(s);
        }
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
        self.check_controls(s);
        // Exercise the total-liability boundary through the actual nested
        // management cache, then restore the fixture before its real topup.
        let key = [
            b"native-async/v1/task/".as_slice(),
            self.contract.as_bytes(),
            b"/",
            &self.task_id.to_be_bytes(),
        ]
        .concat();
        let original = s.get(&key).unwrap();
        let mut liability: TaskFunding = cosmwasm_std::from_json(&original).unwrap();
        liability.available = Uint128::MAX - liability.reserved;
        s.set(&key, &cosmwasm_std::to_json_vec(&liability).unwrap());
        let before: Vec<_> = s
            .range(None, None, cosmwasm_std::Order::Ascending)
            .collect();
        let transfers = self.inner.transfers;
        assert!(self
            .runtime
            .manage(
                self.api,
                s,
                2,
                &self.contract,
                SchedulerMsg::FundTask {
                    task_id: self.task_id,
                    contributor: self.contract.to_string(),
                    funds: Coin::new(1u128, "ugas"),
                },
                &mut self.inner
            )
            .is_err());
        assert_eq!(
            self.inner.transfers, transfers,
            "overflow must precede bank movement"
        );
        assert_eq!(
            before,
            s.range(None, None, cosmwasm_std::Order::Ascending)
                .collect::<Vec<_>>()
        );
        s.set(&key, &original);
        self.runtime.manage(
            self.api,
            s,
            2,
            &self.contract,
            SchedulerMsg::FundTask {
                task_id: self.task_id,
                contributor: self.contract.to_string(),
                funds: Coin::new(self.deposit.u128(), "ugas"),
            },
            &mut self.inner,
        )?;
        Ok(Metered {
            gas_used: self.dispatch_gas,
            result: if self.inner.dispatch_fail {
                Err(PauseReason::MessageFailure)
            } else {
                Ok(())
            },
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
        other_task: None,
        check_during_poll: true,
        deposit: Uint128::new(50),
        dispatch_gas: 0,
    };
    rt.run_block(&api, &mut store, 2, &mut reentrant).unwrap();
    assert_no_flight_markers(&store);
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

fn assert_no_flight_markers(s: &dyn Storage) {
    let storage_has_prefix = |prefix: &[u8]| {
        s.range(None, None, cosmwasm_std::Order::Ascending)
            .any(|(key, _)| key.starts_with(prefix))
    };
    assert!(!storage_has_prefix(b"na2/transient/flight/"));
}

#[test]
fn zero_price_reentrant_controls_are_rejected() {
    reentrant_outcomes_reject_controls_and_conserve_funding(true);
}

#[test]
fn nested_funding_preserves_reserved_liability() {
    reentrant_outcomes_reject_controls_and_conserve_funding(false);
}

// Removing the flight guard must allow a zero-price nested cancellation;
// dropping the funding reload must lose the nested deposit from settlement.
fn reentrant_outcomes_reject_controls_and_conserve_funding(zero_price: bool) {
    for outcome in [
        PollOutcome::Pending {
            state: Binary::default(),
            wait: Wait::Height { height: 4 },
        },
        PollOutcome::Yield {
            state: Binary::default(),
        },
        PollOutcome::Complete {
            output: Binary::default(),
        },
        PollOutcome::Fail {
            error: "business failure".into(),
        },
    ] {
        let terminal = matches!(
            outcome,
            PollOutcome::Complete { .. } | PollOutcome::Fail { .. }
        );
        let (mut rt, api, mut store, c, mut host) = setup();
        rt.config.block_gas = 40; // One poll; the independent task's indexes stay intact.
        if zero_price {
            rt.keeper.config.gas_price = Decimal::zero();
            rt.keeper.config.min_base_price = Decimal::zero();
        }
        let id = register(&rt, &api, &mut store, &c, &mut host);
        let other = register(&rt, &api, &mut store, &c, &mut host);
        let available_before = funding(&rt, &api, &store, &c, id).available;
        let balance_before: Uint128 =
            cosmwasm_std::from_json(store.get(format!("bank/{c}").as_bytes()).unwrap()).unwrap();
        host.outcome = outcome;
        let mut reentrant = ReentrantHost {
            inner: host,
            runtime: &rt,
            api: &api,
            contract: c.clone(),
            task_id: id,
            node_fault: false,
            other_task: Some(other),
            check_during_poll: true,
            deposit: Uint128::new(50),
            dispatch_gas: 0,
        };
        assert_eq!(
            rt.run_block(&api, &mut store, 2, &mut reentrant)
                .unwrap()
                .polls,
            1
        );
        assert_no_flight_markers(&store);
        let funding_after = funding(&rt, &api, &store, &c, id);
        let paid = Uint128::new(if zero_price { 0 } else { 9 });
        assert_eq!(funding_after.cumulative_execution_fees, paid);
        assert_eq!(funding_after.reserved, Uint128::zero());
        if terminal {
            assert_eq!(funding_after.available, Uint128::zero());
            let balance_after: Uint128 =
                cosmwasm_std::from_json(store.get(format!("bank/{c}").as_bytes()).unwrap())
                    .unwrap();
            assert_eq!(balance_after, balance_before + available_before - paid);
        } else {
            let deposit = Uint128::new(50);
            assert_eq!(funding_after.available, available_before + deposit - paid);
            // Management succeeds after the attempt, even at zero price.
            rt.execute(
                &api,
                &mut store,
                2,
                &c,
                RuntimeMsg::ExtendLease {
                    task_id: id,
                    actor: c.to_string(),
                    allocated_bytes: 5000,
                    expires_at: 25,
                },
                true,
                &mut reentrant.inner,
            )
            .unwrap();
            set_bid_at(
                &rt,
                &api,
                &mut store,
                &c,
                &mut reentrant.inner,
                2,
                id,
                40,
                "1",
            );
            rt.manage(
                &api,
                &mut store,
                2,
                &c,
                SchedulerMsg::CancelTask {
                    task_id: id,
                    actor: c.to_string(),
                },
                &mut reentrant.inner,
            )
            .unwrap();
            assert_no_flight_markers(&store);
        }
        let untouched = rt.task(&store, &c, other, 2).unwrap();
        assert_eq!(untouched.expires_at, 20);
        assert_eq!(untouched.allocated_bytes, 4096);
        assert_eq!(untouched.last_attempt, None);
    }
}

#[test]
fn failed_nested_funding_rolls_back_and_releases_flight() {
    for failure in [
        "dispatch",
        "dispatch_oog",
        "poll_oog",
        "poll_trap",
        "host_error",
    ] {
        let (rt, api, mut store, c, mut host) = setup();
        let id = register(&rt, &api, &mut store, &c, &mut host);
        host.outcome = PollOutcome::Yield {
            state: Binary::default(),
        };
        host.dispatch_fail = failure == "dispatch";
        host.fail = failure == "poll_trap";
        if failure == "poll_oog" {
            host.gas = 39;
        }
        let mut reentrant = ReentrantHost {
            inner: host,
            runtime: &rt,
            api: &api,
            contract: c.clone(),
            task_id: id,
            node_fault: failure == "host_error",
            other_task: None,
            check_during_poll: true,
            deposit: Uint128::new(50),
            dispatch_gas: if failure == "dispatch_oog" { 32 } else { 0 },
        };
        let before: Vec<_> = store
            .range(None, None, cosmwasm_std::Order::Ascending)
            .collect();
        let result = rt.run_block(&api, &mut store, 2, &mut reentrant);
        let funds = funding(&rt, &api, &store, &c, id);
        assert_no_flight_markers(&store);
        assert!(store.get(b"user/write").is_none());
        let balance: Uint128 =
            cosmwasm_std::from_json(store.get(format!("bank/{c}").as_bytes()).unwrap()).unwrap();
        assert_eq!(balance, Uint128::new(9000));
        if failure == "host_error" {
            assert!(result.is_err());
            assert_eq!(
                before,
                store
                    .range(None, None, cosmwasm_std::Order::Ascending)
                    .collect::<Vec<_>>()
            );
        } else {
            assert_eq!(result.unwrap().polls, 1);
            let paid = Uint128::new(if failure.ends_with("oog") { 40 } else { 9 });
            assert_eq!(funds.available, Uint128::new(999) - paid);
            assert_eq!(funds.cumulative_execution_fees, paid);
            assert_eq!(funds.reserved, Uint128::zero());
            rt.manage(
                &api,
                &mut store,
                2,
                &c,
                SchedulerMsg::ResumeTask {
                    task_id: id,
                    actor: c.to_string(),
                },
                &mut reentrant.inner,
            )
            .unwrap();
            assert_no_flight_markers(&store);
        }
    }
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
        other_task: None,
        check_during_poll: true,
        deposit: Uint128::new(50),
        dispatch_gas: 0,
    };
    assert!(rt.run_block(&api, &mut store, 2, &mut reentrant).is_err());
    assert_no_flight_markers(&store);
    assert_eq!(rt.task(&store, &c, id, 2).unwrap().charged, Uint128::zero());
    assert!(store.get(b"user/write").is_none());
    reentrant.node_fault = false;
    assert_eq!(
        rt.run_block(&api, &mut store, 2, &mut reentrant)
            .unwrap()
            .polls,
        1
    );
    assert_no_flight_markers(&store);
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
fn cancellation_churn_removes_bid_entry_before_releasing_storage() {
    let (mut rt, api, mut s, c, mut host) = setup();
    let id = register(&rt, &api, &mut s, &c, &mut host);
    // Promote without polling, leaving a bid entry to clean up.
    rt.config.max_visits = 0;
    rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    assert_eq!(
        s.range(
            Some(b"na2/bid/"),
            Some(b"na2/bid0"),
            cosmwasm_std::Order::Ascending
        )
        .count(),
        1
    );
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
    rt.run_block(&api, &mut s, 22, &mut host).unwrap();
    for prefix in [
        b"na2/ring".as_slice(),
        b"na2/member/",
        b"na2/ready/",
        b"na2/bid/",
        b"na2/due/",
    ] {
        assert!(
            !s.range(None, None, cosmwasm_std::Order::Ascending)
                .any(|(k, _)| k.starts_with(prefix)),
            "released task left queue storage behind"
        );
    }
}

#[test]
fn last_active_task_removes_owner_count_and_pruning_keeps_ids() {
    let (rt, api, mut s, a, mut host) = setup();
    let b = api.addr_make("other-contract");
    s.set(
        format!("bank/{b}").as_bytes(),
        &cosmwasm_std::to_json_binary(&Uint128::new(10000)).unwrap(),
    );
    let a1 = register(&rt, &api, &mut s, &a, &mut host);
    let a2 = register(&rt, &api, &mut s, &a, &mut host);
    let b1 = register(&rt, &api, &mut s, &b, &mut host);
    let count = |c: &Addr| [b"na2/count/".as_slice(), c.as_bytes()].concat();
    for (owner, id) in [(&a, a1), (&b, b1), (&a, a2)] {
        rt.manage(
            &api,
            &mut s,
            2,
            owner,
            SchedulerMsg::CancelTask {
                task_id: id,
                actor: owner.to_string(),
            },
            &mut host,
        )
        .unwrap();
        if owner == a && id == a1 {
            assert_eq!(
                cosmwasm_std::from_json::<u64>(s.get(&count(&a)).unwrap()).unwrap(),
                1
            );
        } else {
            assert!(
                s.get(&count(owner)).is_none(),
                "last task left an empty owner count"
            );
        }
    }
    assert_eq!(rt.run_block(&api, &mut s, 22, &mut host).unwrap().prunes, 3);
    for prefix in [
        b"na2/count/".as_slice(),
        b"na2/task/",
        b"na2/prune/",
        b"na2/expiry/",
        b"na2/ring/",
        b"na2/member/",
        b"na2/ready/",
        b"na2/bid/",
        b"na2/due/",
        b"native-async/v1/task/",
    ] {
        assert!(
            !s.range(None, None, cosmwasm_std::Order::Ascending)
                .any(|(key, _)| key.starts_with(prefix)),
            "leftover index {}",
            String::from_utf8_lossy(prefix)
        );
    }
    assert_eq!(register(&rt, &api, &mut s, &a, &mut host), a2 + 1);
    assert_eq!(register(&rt, &api, &mut s, &b, &mut host), b1 + 1);
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
    task.ready_at = u64::MAX;
    funding.cumulative_execution_fees = task.charged;
    s.set(&task_key, &cosmwasm_std::to_json_vec(&task).unwrap());
    s.set(&funding_key, &cosmwasm_std::to_json_vec(&funding).unwrap());
    host.fail = true;
    assert_eq!(rt.run_block(&api, &mut s, 2, &mut host).unwrap().polls, 1);
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

#[test]
fn exact_quota_preallocates_maximum_readiness_and_bid_metadata() {
    let quota = minimum_quota();
    let (mut rt, api, mut s, c, mut host) = setup();
    let id = register_with_quota(&rt, &api, &mut s, &c, &mut host, quota).unwrap();
    rt.config.max_visits = 0;
    rt.run_block(&api, &mut s, 2, &mut host).unwrap();
    let mut task = rt.task(&s, &c, id, 2).unwrap();
    let value = serde_json::to_value(&task).unwrap();
    let queue_key: Binary = serde_json::from_value(value["queue_key"].clone()).unwrap();
    assert_eq!(queue_key.len(), 56); // prefix + 32 hex digits + two u64s
                                     // Check the admitted record's largest mandatory metadata encoding while
                                     // retaining its real bid key, rather than relying on slack after unqueue.
    task.ready_at = u64::MAX;
    task.wait_sequence = u64::MAX;
    task.last_attempt = Some(u64::MAX);
    task.charged = Uint128::MAX;
    task.reserved = Uint128::MAX;
    task.storage_fee = Uint128::MAX;
    task.allocated_bytes = u64::MAX;
    task.expires_at = u64::MAX;
    task.retain_until = u64::MAX;
    task.status = ExecutionStatus::Completed;
    let mut funds = funding(&rt, &api, &s, &c, id);
    funds.available = Uint128::MAX;
    funds.reserved = Uint128::MAX;
    funds.cumulative_execution_fees = Uint128::MAX;
    funds.max_attempt_charge = Uint128::MAX;
    funds.storage_lease_paid = Uint128::MAX;
    funds.allocated_bytes = u64::MAX;
    funds.expires_at = u64::MAX;
    funds.status = TaskStatus::Paused {
        reason: PauseReason::InsufficientFunds,
        resume: ResumeState::Runnable,
    };
    let bytes = cosmwasm_std::to_json_vec(&task).unwrap().len()
        + cosmwasm_std::to_json_vec(&funds).unwrap().len();
    assert!(bytes + 1024 <= quota as usize);
    // Older records remain readable with the serde-defaulted readiness field.
    let mut old = serde_json::to_value(task).unwrap();
    old.as_object_mut().unwrap().remove("ready_at");
    assert_eq!(
        serde_json::from_value::<ExecutionTask>(old)
            .unwrap()
            .ready_at,
        0
    );
}

#[test]
fn shared_go_rust_result_boundaries_use_payload_bytes_and_preserve_rejected_state() {
    #[derive(serde::Deserialize)]
    struct Boundary {
        name: String,
        max_result_bytes: u32,
        resolution: Resolution,
        accepted: bool,
    }
    let cases: Vec<Boundary> = serde_json::from_slice(include_bytes!(
        "../../../integration/wasmd/testdata/result-boundaries.json"
    ))
    .unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let (rt, api, mut store, contract, mut host) = setup();
        let id = register(&rt, &api, &mut store, &contract, &mut host);
        host.outcome = PollOutcome::Pending {
            state: Binary::from(b"checkpoint".as_slice()),
            wait: Wait::Operation {
                resolver: TEST_RESOLVER.into(),
                deadline: 10,
                max_result_bytes: case.max_result_bytes,
            },
        };
        rt.run_block(&api, &mut store, 2, &mut host).unwrap();
        let before: Vec<_> = store
            .range(None, None, cosmwasm_std::Order::Ascending)
            .collect();
        let delivered = rt.deliver(
            &mut store,
            3,
            &contract,
            id,
            1,
            TEST_RESOLVER,
            case.resolution.clone(),
            false,
        );
        assert_eq!(delivered.is_ok(), case.accepted, "{}", case.name);
        if case.accepted {
            assert_eq!(
                rt.task(&store, &contract, id, 3).unwrap().inbox,
                Some(case.resolution.clone()),
                "{}",
                case.name
            );
            host.outcome = PollOutcome::Complete {
                output: Binary::default(),
            };
            assert_eq!(
                rt.run_block(&api, &mut store, 3, &mut host).unwrap().polls,
                1
            );
            let request: PollRequest =
                cosmwasm_std::from_json(store.get(b"user/request").unwrap()).unwrap();
            assert_eq!(
                request.step,
                PollStep::Resume {
                    state: Binary::from(b"checkpoint".as_slice()),
                    resolution: case.resolution,
                },
                "{}",
                case.name
            );
        } else {
            let after: Vec<_> = store
                .range(None, None, cosmwasm_std::Order::Ascending)
                .collect();
            assert_eq!(after, before, "{}", case.name);
            assert_eq!(
                rt.run_block(&api, &mut store, 3, &mut host).unwrap().polls,
                0
            );
        }
    }
}

#[test]
fn delivery_marks_funding_runnable_in_both_phases() {
    for scheduler_phase in [false, true] {
        let (rt, api, mut store, c, mut host) = setup();
        let id = register(&rt, &api, &mut store, &c, &mut host);
        host.outcome = PollOutcome::Pending {
            state: Binary::from(b"saved".as_slice()),
            wait: Wait::Operation {
                resolver: TEST_RESOLVER.into(),
                deadline: 5,
                max_result_bytes: 30,
            },
        };
        rt.run_block(&api, &mut store, 2, &mut host).unwrap();
        assert_eq!(
            funding(&rt, &api, &store, &c, id).status,
            TaskStatus::Waiting
        );
        let good = Resolution::Success {
            data: Binary::from(b"yes".as_slice()),
        };
        rt.deliver(
            &mut store,
            3,
            &c,
            id,
            1,
            TEST_RESOLVER,
            good,
            scheduler_phase,
        )
        .unwrap();
        assert_eq!(
            funding(&rt, &api, &store, &c, id).status,
            TaskStatus::Runnable,
            "scheduler_phase={scheduler_phase}"
        );
        assert_eq!(
            rt.task(&store, &c, id, 3).unwrap().status,
            ExecutionStatus::Runnable,
            "scheduler_phase={scheduler_phase}"
        );
    }
}

fn bank(store: &dyn Storage, who: &Addr) -> Uint128 {
    store
        .get(format!("bank/{who}").as_bytes())
        .map(|v| cosmwasm_std::from_json(v).unwrap())
        .unwrap_or_default()
}

#[test]
fn rejected_refund_never_blocks_scheduler_terminal_transitions() {
    // Completion in a poll, and expiry in maintenance.
    for expire in [false, true] {
        let (rt, api, mut store, c, mut host) = setup();
        let id = register(&rt, &api, &mut store, &c, &mut host);
        if expire {
            host.outcome = PollOutcome::Pending {
                state: Binary::from(b"saved".as_slice()),
                wait: Wait::Height { height: 19 },
            };
            rt.run_block(&api, &mut store, 2, &mut host).unwrap();
        }
        store.set(format!("bank/blocked/{c}").as_bytes(), b"1");
        let escrow_before = bank(&store, &rt.keeper.escrow);
        let height = if expire { 20 } else { 2 };
        let report = rt.run_block(&api, &mut store, height, &mut host).unwrap();
        assert_eq!(report.stranded_refunds, 1, "expire={expire}");
        let task = rt.task(&store, &c, id, height).unwrap();
        let expected = if expire {
            ExecutionStatus::Expired
        } else {
            ExecutionStatus::Completed
        };
        assert_eq!(task.status, expected);
        let funds = funding(&rt, &api, &store, &c, id);
        assert!(funds.available.is_zero());
        // The unrefunded escrow stays in the escrow account; only fees left it.
        assert!(bank(&store, &rt.keeper.escrow) >= escrow_before - task.charged);
        assert!(!bank(&store, &rt.keeper.escrow).is_zero());
        // Later blocks keep making progress.
        rt.run_block(&api, &mut store, height + 1, &mut host)
            .unwrap();
    }
}

#[test]
fn rejected_refund_fails_user_cancellation_without_side_effects() {
    let (rt, api, mut store, c, mut host) = setup();
    let id = register(&rt, &api, &mut store, &c, &mut host);
    store.set(format!("bank/blocked/{c}").as_bytes(), b"1");
    let before = snapshot(&store);
    assert!(rt
        .execute(
            &api,
            &mut store,
            1,
            &c,
            management(serde_json::json!({"funding": {"cancel_task": {"task_id": id, "actor": c.to_string()}}})),
            false,
            &mut host,
        )
        .is_err());
    assert_eq!(snapshot(&store), before);
}

#[test]
fn rejected_refund_falls_back_to_control_then_owner() {
    for blocked in 0..4 {
        let (rt, api, mut store, c, mut host) = setup();
        let (refund, control) = (api.addr_make("refund"), api.addr_make("control"));
        let id = rt
            .register(
                &api,
                &mut store,
                1,
                &c,
                SchedulerMsg::RegisterTask {
                    control: control.to_string(),
                    refund: refund.to_string(),
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
                &mut host,
            )
            .unwrap();
        let order = [&refund, &control, &c];
        for who in &order[..blocked] {
            store.set(format!("bank/blocked/{who}").as_bytes(), b"1");
        }
        let before: Vec<Uint128> = order.iter().map(|a| bank(&store, a)).collect();
        let report = rt.run_block(&api, &mut store, 2, &mut host).unwrap();
        assert_eq!(
            report.redirected_refunds,
            u32::from(blocked == 1 || blocked == 2)
        );
        assert_eq!(report.stranded_refunds, u32::from(blocked == 3));
        for (i, who) in order.iter().enumerate() {
            let gained = bank(&store, who) - before[i];
            assert_eq!(!gained.is_zero(), i == blocked, "blocked={blocked} i={i}");
        }
        assert_eq!(
            funding(&rt, &api, &store, &c, id).refund,
            refund.to_string()
        );
    }
}
