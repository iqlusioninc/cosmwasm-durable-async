use cosmwasm_std::{coin, coins, Addr, CosmosMsg, MessageInfo};
use native_async::{fund_task, SchedulerMsg};

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
