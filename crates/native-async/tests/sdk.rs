use cosmwasm_std::{coin, coins, Addr, CosmosMsg, MessageInfo};
use native_async::{fund_task, SchedulerMsg};

#[test]
fn lease_and_budget_forward_authenticated_actor_and_reject_funds() {
    let mut info = MessageInfo {
        sender: Addr::unchecked("sender"),
        funds: vec![],
    };
    let lease = native_async::extend_lease(&info, 7, 8192, 1000).unwrap();
    assert_eq!(
        lease.messages[0].msg,
        CosmosMsg::Custom(native_async::RuntimeMsg::ExtendLease {
            task_id: 7,
            actor: "sender".into(),
            allocated_bytes: 8192,
            expires_at: 1000,
        })
    );
    let budget =
        native_async::update_budget(&info, 7, 100, cosmwasm_std::Decimal::one(), "refund".into())
            .unwrap();
    assert_eq!(
        budget.messages[0].msg,
        CosmosMsg::Custom(native_async::RuntimeMsg::UpdateBudget {
            task_id: 7,
            actor: "sender".into(),
            gas_per_poll: 100,
            max_gas_price: cosmwasm_std::Decimal::one(),
            refund: "refund".into(),
        })
    );
    info.funds = coins(1, "ugas");
    assert!(native_async::extend_lease(&info, 7, 8192, 1000).is_err());
    assert!(native_async::update_budget(
        &info,
        7,
        100,
        cosmwasm_std::Decimal::one(),
        "refund".into()
    )
    .is_err());
}

#[test]
fn forwards_only_attached_gas_funds_and_contributor() {
    let info = MessageInfo {
        sender: Addr::unchecked("sponsor"),
        funds: coins(17, "ugas"),
    };
    let response = fund_task(&info, 4, "ugas").unwrap();
    assert_eq!(response.messages.len(), 1);
    assert_eq!(
        response.messages[0].msg,
        CosmosMsg::Custom(SchedulerMsg::FundTask {
            task_id: 4,
            contributor: "sponsor".into(),
            funds: coin(17, "ugas"),
        })
    );
}

#[test]
fn rejects_ambiguous_or_invalid_deposits() {
    for funds in [
        vec![],
        coins(0, "ugas"),
        coins(1, "other"),
        vec![coin(1, "ugas"), coin(1, "other")],
        vec![coin(1, "ugas"), coin(2, "ugas")],
    ] {
        let info = MessageInfo {
            sender: Addr::unchecked("sponsor"),
            funds,
        };
        assert!(fund_task(&info, 1, "ugas").is_err());
    }
}
