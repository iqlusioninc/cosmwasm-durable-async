mod support;
use cosmwasm_std::*;
use cw_multi_test::Executor;
use native_async::runtime::PollHost;
use native_async::*;
use support::Suite;

#[test]
fn native_adapter_delivers_at_next_block() {
    // Catches omitted staging/dispatch and scheduler-phase Deliver routing.
    let mut suite = Suite::new();
    suite.register(false);
    assert_eq!(suite.block(1).unwrap().polls, 0);
    let report = suite.block(2).unwrap();
    assert_eq!(report.polls, 1);
    // A callback during dispatch must be queued for H+1, not revisited at H.
    assert_eq!(report.visits, 1);
    let task = suite.task();
    assert!(task.inbox.is_some());
    assert_eq!(task.last_attempt, Some(2));
    let repeated = suite.block(2).unwrap();
    assert_eq!(repeated.polls, 0);
    assert_eq!(repeated.visits, 0);
    assert_eq!(
        suite.business_balances(),
        vec![coin(90, "uinput"), coin(10, "uinput")]
    );
    assert_eq!(suite.block(3).unwrap().polls, 1);
    assert_eq!(suite.task().status, TaskStatus::Completed);
    assert!(suite
        .app
        .storage()
        .get(support::poll_host::PHASE_KEY)
        .is_none());
}

#[test]
fn native_adapter_conserves_fees_using_fixed_logical_gas() {
    let mut suite = Suite::new();
    suite.register(false);
    let first = suite.block(2).unwrap();
    // Hand-calculated: host 2 + poll 7 + one returned message 5 = 14.
    assert_eq!(first.base_price, Decimal::one());
    assert_eq!(first.gas_used, 14);
    assert_eq!(first.poll_gas_used, 12);
    // Registration separately pays the fixed one-coin cleanup/storage charge.
    assert_eq!(suite.funding().storage_lease_paid, Uint128::one());
    assert_eq!(suite.funding().available, Uint128::new(985));
    assert_eq!(suite.funding().reserved, Uint128::zero());
    assert_eq!(suite.funding().cumulative_execution_fees, Uint128::new(14));
    assert_eq!(
        suite
            .app
            .wrap()
            .query_balance(&suite.escrow, "ugas")
            .unwrap(),
        coin(985, "ugas")
    );
    assert_eq!(
        suite.app.wrap().query_balance(&suite.fees, "ugas").unwrap(),
        coin(15, "ugas")
    );
    let second = suite.block(3).unwrap();
    // Completion has no dispatch messages: 2 + 7 = 9. Floor remains positive.
    assert_eq!(second.base_price, Decimal::one());
    assert_eq!(second.gas_used, 9);
    assert_eq!(suite.funding().cumulative_execution_fees, Uint128::new(23));
    assert_eq!(suite.funding().available, Uint128::zero());
    assert_eq!(
        suite
            .app
            .wrap()
            .query_balance(&suite.escrow, "ugas")
            .unwrap(),
        coin(0, "ugas")
    );
    assert_eq!(
        suite.app.wrap().query_balance(&suite.fees, "ugas").unwrap(),
        coin(24, "ugas")
    );
    assert_eq!(
        suite
            .app
            .wrap()
            .query_balance(&suite.owner, "ugas")
            .unwrap(),
        coin(99_976, "ugas")
    );
}

#[test]
fn native_adapter_rejects_account_custom_messages() {
    let mut suite = Suite::new();
    // Supply the later immutable identity check so removing the provenance
    // boundary would actually admit this otherwise valid registration.
    let owner = suite.owner.clone();
    suite.app.init_modules(|router, _, _| {
        router.custom.identities.insert(
            owner.to_string(),
            Binary::from(b"known-account-test-identity".as_slice()),
        );
    });
    let before = suite
        .app
        .wrap()
        .query_balance(&suite.owner, "ugas")
        .unwrap();
    let result = suite.app.execute(
        suite.owner.clone(),
        CosmosMsg::Custom(RuntimeMsg::Register {
            control: suite.owner.to_string(),
            refund: suite.owner.to_string(),
            gas_per_poll: 100,
            max_gas_price: Decimal::one(),
            expires_at: 100,
            funds: coin(1_000, "ugas"),
            program: TaskProgram {
                kind: "forged".into(),
                version: 1,
                input: Binary::default(),
            },
            allocated_bytes: 20_000,
        }),
    );
    let error = result.unwrap_err();
    assert!(format!("{error:#}").contains("ContractData"), "{error:#}");
    assert_eq!(
        suite
            .app
            .wrap()
            .query_balance(&suite.owner, "ugas")
            .unwrap(),
        before
    );
}

#[test]
fn native_adapter_surfaces_infrastructure_errors_and_rolls_back_block() {
    let mut suite = Suite::new();
    suite.register(false);
    let before: Vec<_> = suite
        .app
        .storage()
        .range(None, None, Order::Ascending)
        .collect();
    suite
        .app
        .init_modules(|router, _, _| router.custom.identities.clear());
    let error = suite.block(2).unwrap_err();
    assert!(error
        .to_string()
        .contains("unknown immutable Rust fixture identity"));
    let after: Vec<_> = suite
        .app
        .storage()
        .range(None, None, Order::Ascending)
        .collect();
    assert_eq!(after, before);
    assert_eq!(suite.task().last_attempt, None);
}

#[test]
fn native_adapter_restores_existing_phase_marker_on_success_and_error() {
    let mut suite = Suite::new();
    let block = suite.app.block_info();
    let contract = suite.contract.clone();
    suite.app.init_modules(|router, api, storage| {
        storage.set(support::poll_host::PHASE_KEY, b"outer-marker");
        let mut host = support::poll_host::RouterPollHost {
            api,
            router,
            block: &block,
            identities: &router.custom.identities,
        };
        assert!(host
            .dispatch(storage, contract.as_str(), &[], 100)
            .unwrap()
            .result
            .is_ok());
        assert_eq!(
            storage.get(support::poll_host::PHASE_KEY),
            Some(b"outer-marker".to_vec())
        );
        let invalid = WasmMsg::Execute {
            contract_addr: api.addr_make("missing").to_string(),
            msg: Binary::default(),
            funds: vec![],
        }
        .into();
        assert!(host
            .dispatch(storage, contract.as_str(), &[invalid], 100)
            .is_err());
        assert_eq!(
            storage.get(support::poll_host::PHASE_KEY),
            Some(b"outer-marker".to_vec())
        );
        storage.remove(support::poll_host::PHASE_KEY);
    });
}

#[test]
fn native_adapter_rolls_back_nested_failure() {
    // Catches using app-owned rather than runtime-supplied transactional storage.
    let mut suite = Suite::new();
    suite.register(true);
    let before = suite.business_balances();
    let report = suite.block(2).unwrap();
    assert_eq!(report.polls, 1);
    assert_eq!(report.gas_used, 14);
    assert_eq!(suite.funding().cumulative_execution_fees, Uint128::new(14));
    assert_eq!(suite.funding().available, Uint128::new(985));
    assert_eq!(
        suite.app.wrap().query_balance(&suite.fees, "ugas").unwrap(),
        coin(15, "ugas")
    );
    assert_eq!(suite.business_balances(), before);
    assert_eq!(suite.writes(), (None, None, None));
    assert!(matches!(
        suite.task().status,
        TaskStatus::Paused {
            reason: PauseReason::MessageFailure,
            ..
        }
    ));
    assert_eq!(suite.task().last_attempt, Some(2));
    assert_eq!(suite.block(2).unwrap().polls, 0);
    assert!(suite
        .app
        .storage()
        .get(support::poll_host::PHASE_KEY)
        .is_none());
}

#[test]
fn native_adapter_reconstructs_same_state() {
    // Catches missing durable state or missing rebuilt code identities.
    let mut suite = Suite::new();
    suite.register(false);
    assert_eq!(suite.block(2).unwrap().polls, 1);
    let before = (
        suite.task(),
        suite.funding(),
        suite.business_balances(),
        suite.writes(),
    );
    suite.reconstruct().unwrap();
    assert_eq!(
        (
            suite.task(),
            suite.funding(),
            suite.business_balances(),
            suite.writes()
        ),
        before
    );
    assert_eq!(suite.block(2).unwrap().polls, 0);
    assert_eq!(suite.block(3).unwrap().polls, 1);
    assert_eq!(suite.task().status, TaskStatus::Completed);
}
