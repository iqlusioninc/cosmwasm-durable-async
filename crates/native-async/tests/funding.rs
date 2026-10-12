mod support;
use cosmwasm_std::*;
use cw_multi_test::Executor;
use native_async::*;
use support::*;

#[test]
fn legacy_keeper_still_rejects_zero_fixed_price() {
    let mut s = Suite::new();
    s.app
        .init_modules(|router, _, _| router.custom.keeper.config.gas_price = Decimal::zero());
    assert!(s
        .app
        .wrap()
        .query_wasm_smart::<FundingConfig>(&s.contract, &AsyncQueryMsg::FundingConfig {},)
        .is_err());
    assert!(s
        .app
        .execute_contract(
            s.owner.clone(),
            s.contract.clone(),
            &Execute::Register {
                gas_funds: Uint128::new(40),
            },
            &coins(40, "ugas")
        )
        .is_err());
    assert_eq!(s.balance(&s.owner), 10_000);
    assert_eq!(s.balance(&s.contract), 0);
    assert_eq!(s.balance(&s.escrow), 0);
}

#[test]
fn failed_refunds_preserve_task_escrow_and_recipient_balance() {
    let mut s = Suite::new();
    s.register(40, 40);
    let before = s.task();
    // A successful refund must still roll back if the enclosing transaction fails.
    assert!(s
        .app
        .execute_multi(
            s.owner.clone(),
            vec![
                WasmMsg::Execute {
                    contract_addr: s.contract.to_string(),
                    msg: to_json_binary(&Execute::Async(AsyncExecuteMsg::CancelTask {
                        task_id: 1
                    }))
                    .unwrap(),
                    funds: vec![],
                }
                .into(),
                BankMsg::Send {
                    to_address: s.sponsor.to_string(),
                    amount: coins(1_000_000, "ugas")
                }
                .into(),
            ]
        )
        .is_err());
    assert_eq!(s.task(), before);
    assert_eq!(s.balance(&s.escrow), 40);
    assert_eq!(s.balance(&s.owner), 9_960);

    // Fault injection at the trusted bank boundary: force the refund transfer
    // to fail. There is no public API that can drain this module account.
    s.app.init_modules(|router, _, storage| {
        router
            .bank
            .init_balance(storage, &s.escrow, vec![])
            .unwrap()
    });
    assert!(s
        .execute(
            s.owner.clone(),
            AsyncExecuteMsg::CancelTask { task_id: 1 },
            &[]
        )
        .is_err());
    assert_eq!(s.task(), before);
    assert_eq!(s.balance(&s.escrow), 0);
    assert_eq!(s.balance(&s.owner), 9_960);
}

#[test]
fn registration_separates_business_funds_and_sponsor_topup_preserves_authority() {
    let mut s = Suite::new();
    s.register(100, 40);
    assert_eq!(s.balance(&s.contract), 60);
    assert_eq!(s.balance(&s.escrow), 40);
    s.pause(ResumeState::Waiting);
    let receipt = s
        .execute(
            s.sponsor.clone(),
            AsyncExecuteMsg::FundTask { task_id: 1 },
            &coins(17, "ugas"),
        )
        .unwrap();
    let task = s.task();
    assert_eq!(task.available.u128(), 57);
    assert_eq!(s.balance(&s.escrow), 57);
    assert_eq!(s.balance(&s.sponsor), 9_983);
    assert_eq!(task.control, s.owner.as_str());
    assert_eq!(task.refund, s.owner.as_str());
    assert!(matches!(task.status, TaskStatus::Paused { .. }));
    assert!(receipt.events.iter().any(|e| e.ty == "async_funded"
        && e.attributes
            .iter()
            .any(|a| a.key == "contributor" && a.value == s.sponsor.as_str())));
    let config: FundingConfig = s
        .app
        .wrap()
        .query_wasm_smart(&s.contract, &AsyncQueryMsg::FundingConfig {})
        .unwrap();
    assert_eq!(config.denom, "ugas");
    assert_eq!(task.max_attempt_charge, Uint128::new(10));
}

#[test]
fn plain_transfer_is_not_credit_and_failed_deposit_rolls_back_bank_and_ledger() {
    let mut s = Suite::new();
    s.register(40, 40);
    s.app
        .send_tokens(s.sponsor.clone(), s.contract.clone(), &coins(5, "ugas"))
        .unwrap();
    assert_eq!(s.task().available.u128(), 40);
    assert!(s
        .app
        .execute_contract(
            s.sponsor.clone(),
            s.contract.clone(),
            &Execute::FundThenFail { task_id: 1 },
            &coins(17, "ugas")
        )
        .is_err());
    assert_eq!(s.task().available.u128(), 40);
    assert_eq!(s.balance(&s.escrow), 40);
    assert_eq!(s.balance(&s.contract), 5);
    assert_eq!(s.balance(&s.sponsor), 9_995);
}

#[test]
fn owner_can_fund_and_resume_but_waiting_state_and_refund_ownership_survive() {
    let mut s = Suite::new();
    s.register(1, 1);
    s.pause(ResumeState::Waiting);
    assert!(s
        .execute(
            s.sponsor.clone(),
            AsyncExecuteMsg::ResumeTask { task_id: 1 },
            &[]
        )
        .is_err());
    assert!(s
        .execute(
            s.owner.clone(),
            AsyncExecuteMsg::ResumeTask { task_id: 1 },
            &[]
        )
        .is_err());
    s.app
        .execute_multi(
            s.owner.clone(),
            vec![
                WasmMsg::Execute {
                    contract_addr: s.contract.to_string(),
                    msg: to_json_binary(&Execute::Async(AsyncExecuteMsg::FundTask { task_id: 1 }))
                        .unwrap(),
                    funds: coins(20, "ugas"),
                }
                .into(),
                WasmMsg::Execute {
                    contract_addr: s.contract.to_string(),
                    msg: to_json_binary(&Execute::Async(AsyncExecuteMsg::ResumeTask {
                        task_id: 1,
                    }))
                    .unwrap(),
                    funds: vec![],
                }
                .into(),
            ],
        )
        .unwrap();
    assert_eq!(s.task().status, TaskStatus::Waiting);
    s.execute(
        s.sponsor.clone(),
        AsyncExecuteMsg::FundTask { task_id: 1 },
        &coins(9, "ugas"),
    )
    .unwrap();
    assert!(s
        .execute(
            s.sponsor.clone(),
            AsyncExecuteMsg::CancelTask { task_id: 1 },
            &[]
        )
        .is_err());
    s.execute(
        s.owner.clone(),
        AsyncExecuteMsg::CancelTask { task_id: 1 },
        &[],
    )
    .unwrap();
    assert_eq!(s.task().status, TaskStatus::Cancelled);
    assert_eq!(s.task().available, Uint128::zero());
    assert_eq!(s.balance(&s.owner), 10_009);
    assert_eq!(s.balance(&s.escrow), 0);
    assert!(s
        .execute(
            s.owner.clone(),
            AsyncExecuteMsg::FundTask { task_id: 1 },
            &coins(1, "ugas")
        )
        .is_err());
    assert!(s
        .execute(
            s.owner.clone(),
            AsyncExecuteMsg::CancelTask { task_id: 1 },
            &[]
        )
        .is_err());
}

#[test]
fn wrong_contract_unknown_task_expiry_and_wrong_denom_reject_without_credit() {
    let mut s = Suite::new();
    s.register(40, 40);
    assert!(s
        .app
        .execute_contract(
            s.sponsor.clone(),
            s.other.clone(),
            &Execute::Async(AsyncExecuteMsg::FundTask { task_id: 1 }),
            &coins(7, "ugas")
        )
        .is_err());
    assert!(s
        .execute(
            s.sponsor.clone(),
            AsyncExecuteMsg::FundTask { task_id: 2 },
            &coins(7, "ugas")
        )
        .is_err());
    assert!(s
        .execute(
            s.sponsor.clone(),
            AsyncExecuteMsg::FundTask { task_id: 1 },
            &coins(7, "wrong")
        )
        .is_err());
    let expiry = s.task().expires_at;
    s.app.update_block(|b| b.height = expiry);
    assert_eq!(s.task().status, TaskStatus::Expired);
    assert!(s
        .execute(
            s.sponsor.clone(),
            AsyncExecuteMsg::FundTask { task_id: 1 },
            &coins(7, "ugas")
        )
        .is_err());
    assert_eq!(s.balance(&s.sponsor), 10_000);
    assert_eq!(s.balance(&s.escrow), 40);
    // Expiry cannot trap unused funds while cleanup is not implemented.
    s.execute(
        s.owner.clone(),
        AsyncExecuteMsg::CancelTask { task_id: 1 },
        &[],
    )
    .unwrap();
    assert_eq!(s.balance(&s.owner), 10_000);
}

#[test]
fn claimed_amount_cannot_mint_credit_and_accounts_cannot_impersonate_contracts() {
    let mut s = Suite::new();
    s.register(40, 40);
    assert!(s
        .app
        .execute_contract(
            s.sponsor.clone(),
            s.contract.clone(),
            &Execute::ClaimFunds {
                task_id: 1,
                amount: Uint128::new(999)
            },
            &[]
        )
        .is_err());
    assert!(s
        .app
        .execute(
            s.sponsor.clone(),
            CosmosMsg::Custom(SchedulerMsg::FundTask {
                task_id: 1,
                contributor: s.owner.to_string(),
                funds: coin(1, "ugas")
            })
        )
        .is_err());
    assert_eq!(s.task().available.u128(), 40);
    assert_eq!(s.balance(&s.escrow), 40);
}

#[test]
fn overflow_deposit_and_invalid_initial_split_leave_balances_unchanged() {
    let mut s = Suite::new();
    s.app.init_modules(|router, _, storage| {
        router
            .bank
            .init_balance(storage, &s.owner, coins(u128::MAX, "ugas"))
            .unwrap()
    });
    s.register(u128::MAX, u128::MAX);
    assert!(s
        .execute(
            s.sponsor.clone(),
            AsyncExecuteMsg::FundTask { task_id: 1 },
            &coins(1, "ugas")
        )
        .is_err());
    assert_eq!(s.task().available, Uint128::MAX);
    assert_eq!(s.balance(&s.escrow), u128::MAX);
    assert_eq!(s.balance(&s.sponsor), 10_000);
    assert!(s
        .app
        .execute_contract(
            s.sponsor.clone(),
            s.other.clone(),
            &Execute::Register {
                gas_funds: 11u128.into()
            },
            &coins(10, "ugas")
        )
        .is_err());
    assert_eq!(s.balance(&s.other), 0);
    assert_eq!(s.balance(&s.sponsor), 10_000);
}

#[test]
fn management_calls_reject_attached_funds_and_price_limit_blocks_resume() {
    let mut s = Suite::new();
    s.register(40, 40);
    s.pause(ResumeState::Runnable);
    assert!(s
        .execute(
            s.owner.clone(),
            AsyncExecuteMsg::ResumeTask { task_id: 1 },
            &coins(1, "ugas")
        )
        .is_err());
    assert!(s
        .execute(
            s.owner.clone(),
            AsyncExecuteMsg::CancelTask { task_id: 1 },
            &coins(1, "ugas")
        )
        .is_err());
    s.app
        .init_modules(|router, _, _| router.custom.keeper.config.gas_price = Decimal::percent(101));
    s.execute(
        s.sponsor.clone(),
        AsyncExecuteMsg::FundTask { task_id: 1 },
        &coins(200, "ugas"),
    )
    .unwrap();
    assert!(s
        .execute(
            s.owner.clone(),
            AsyncExecuteMsg::ResumeTask { task_id: 1 },
            &[]
        )
        .is_err());
    assert_eq!(s.task().max_attempt_charge, Uint128::new(101));
    assert!(matches!(s.task().status, TaskStatus::Paused { .. }));
}

#[test]
fn price_rounds_up_and_task_keys_isolate_contracts_with_the_same_id() {
    let mut s = Suite::new();
    s.app.init_modules(|router, _, _| {
        router.custom.keeper.config.gas_price = Decimal::from_ratio(1u128, 3u128)
    });
    s.register(33, 33);
    assert_eq!(s.task().max_attempt_charge, Uint128::new(34));
    assert!(matches!(
        s.task().status,
        TaskStatus::Paused {
            reason: PauseReason::InsufficientFunds,
            ..
        }
    ));
    s.app
        .execute_contract(
            s.owner.clone(),
            s.other.clone(),
            &Execute::Register {
                gas_funds: 50u128.into(),
            },
            &coins(50, "ugas"),
        )
        .unwrap();
    s.execute(
        s.sponsor.clone(),
        AsyncExecuteMsg::FundTask { task_id: 1 },
        &coins(1, "ugas"),
    )
    .unwrap();
    s.execute(
        s.owner.clone(),
        AsyncExecuteMsg::ResumeTask { task_id: 1 },
        &[],
    )
    .unwrap();
    assert_eq!(s.task().status, TaskStatus::Runnable);
    let other: TaskFunding = s
        .app
        .wrap()
        .query_wasm_smart(&s.other, &AsyncQueryMsg::TaskFunding { task_id: 1 })
        .unwrap();
    assert_eq!(other.available, Uint128::new(50));
    assert_eq!(s.task().available, Uint128::new(34));
    assert_eq!(s.balance(&s.escrow), 84);
}

#[test]
fn failed_registration_does_not_consume_an_id_or_leave_escrow() {
    let mut s = Suite::new();
    let result = s.app.execute_multi(
        s.owner.clone(),
        vec![
            WasmMsg::Execute {
                contract_addr: s.contract.to_string(),
                msg: to_json_binary(&Execute::Register {
                    gas_funds: 40u128.into(),
                })
                .unwrap(),
                funds: coins(40, "ugas"),
            }
            .into(),
            BankMsg::Send {
                to_address: s.sponsor.to_string(),
                amount: coins(1_000_000, "ugas"),
            }
            .into(),
        ],
    );
    assert!(result.is_err());
    assert_eq!(s.balance(&s.escrow), 0);
    assert_eq!(s.balance(&s.owner), 10_000);
    s.register(40, 40);
    assert_eq!(s.task().task_id, 1);
    s.register(20, 20);
    let second: TaskFunding = s
        .app
        .wrap()
        .query_wasm_smart(&s.contract, &AsyncQueryMsg::TaskFunding { task_id: 2 })
        .unwrap();
    assert_eq!(second.available, Uint128::new(20));
}
