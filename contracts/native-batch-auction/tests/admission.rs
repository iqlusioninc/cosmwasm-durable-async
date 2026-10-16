mod support;
use cosmwasm_std::*;
use native_async::*;
use native_batch_auction::{msg::*, state::*, *};
use support::AuctionSuite;

#[test]
fn create_once_binds_actual_seller_and_keeps_principal_out_of_gas_escrow() {
    // Break: forwarding inventory as gas, accepting another creator, or reusing custody.
    for fee in ["ugas", "uquote", "usale"] {
        let mut s = AuctionSuite::with_fee_denom(fee);
        let r = s.request();
        let f = s.funds(&r);
        assert!(s
            .execute(s.sponsor.clone(), &ExecuteMsg::CreateAuction(r), &f)
            .is_err());
        assert_eq!(
            s.create().unwrap(),
            CreateAuctionResponse {
                auction_id: 1,
                task_id: 1
            }
        );
        assert_eq!(s.auction().config.seller, s.seller);
        assert_eq!(s.balance(&s.contract, "usale"), Uint128::new(50));
        assert_eq!(s.balance(&s.contract, "uquote"), Uint128::zero());
        assert_eq!(s.funding().available, Uint128::new(999));
        assert_eq!(s.funding().refund, s.seller.to_string());
        assert!(s.create().is_err());
    }
}

#[test]
fn exact_funds_and_failed_registration_roll_back_every_balance_and_record() {
    // Break: accepting excess/missing funds or retaining records after failed host admission.
    let mut s = AuctionSuite::new();
    let before = s.balance(&s.seller, "usale");
    for funds in [
        vec![coin(50, "usale")],
        vec![coin(51, "usale"), coin(1000, "ugas")],
        vec![coin(50, "usale"), coin(1001, "ugas")],
        vec![coin(50, "usale"), coin(1000, "ugas"), coin(1, "uquote")],
    ] {
        assert!(s
            .execute(
                s.seller.clone(),
                &ExecuteMsg::CreateAuction(s.request()),
                &funds
            )
            .is_err());
        assert_eq!(s.balance(&s.seller, "usale"), before);
    }
    let mut r = s.request();
    r.terms.expires_at = 2000;
    let f = s.funds(&r);
    assert!(s
        .execute(s.seller.clone(), &ExecuteMsg::CreateAuction(r), &f)
        .is_err());
    assert!(s
        .app
        .contract_storage(&s.contract)
        .get(AUCTION_KEY)
        .is_none());
    assert_eq!(s.balance(&s.contract, "usale"), Uint128::zero());
    assert_eq!(s.balance(&s.contract, "ugas"), Uint128::zero());
    s.app
        .contract_storage_mut(&s.contract)
        .set(support::BAD_REPLY_KEY, b"1");
    assert!(s.create().is_err());
    assert!(s
        .app
        .contract_storage(&s.contract)
        .get(AUCTION_KEY)
        .is_none());
    assert_eq!(s.balance(&s.seller, "usale"), before);
    s.app
        .contract_storage_mut(&s.contract)
        .remove(support::BAD_REPLY_KEY);
    assert_eq!(s.create().unwrap().task_id, 1);
}

#[test]
fn commit_and_reveal_heights_duplicate_identities_and_invalid_reveals_preserve_demand() {
    // Break: off-by-one phase admission or writing demand before digest validation.
    let mut s = AuctionSuite::new();
    s.create().unwrap();
    let b = s.bidder("bidder");
    s.height(9);
    s.commit(&b, 2, 1, 20).unwrap();
    assert!(s.commit(&b, 2, 1, 20).is_err());
    assert!(s.reveal(&b, 2, 1).is_err());
    s.height(10);
    let b2 = s.bidder("other");
    assert!(s.commit(&b2, 2, 1, 20).is_err());
    let before = s.auction().auction;
    for (q, t) in [(3, 1), (0, 1), (6, 1), (2, 3)] {
        assert!(s.reveal(&b, q, t).is_err());
        assert_eq!(s.auction().auction, before);
    }
    let mut reveal = RevealBidRequest {
        quantity_lots: Uint128::new(2),
        price_tick: 1,
        beneficiary: s.beneficiary.to_string(),
        salt: Binary::from(vec![7; 31]),
    };
    assert!(s
        .execute(b.clone(), &ExecuteMsg::RevealBid(reveal.clone()), &[])
        .is_err());
    reveal.salt = Binary::from(vec![7; 32]);
    reveal.beneficiary = reveal.beneficiary.to_uppercase();
    assert!(s
        .execute(b.clone(), &ExecuteMsg::RevealBid(reveal), &[])
        .is_err());
    assert_eq!(
        load_demand(&*s.app.contract_storage(&s.contract), 1).unwrap(),
        Uint256::zero()
    );
    s.height(19);
    s.reveal(&b, 2, 1).unwrap();
    assert_eq!(s.auction().auction.total_demand, Uint256::from(2u128));
    assert_eq!(
        load_demand(&*s.app.contract_storage(&s.contract), 1).unwrap(),
        Uint256::from(2u128)
    );
    assert!(s.reveal(&b, 2, 1).is_err());
    s.height(20);
    assert!(s.reveal(&b, 2, 1).is_err());
    assert_eq!(s.balance(&s.contract, "uquote"), Uint128::new(20));
}

#[test]
fn undercollateralized_reveal_and_full_book_do_not_accept_new_liabilities() {
    // Break: skipping price multiplication or bid-count admission.
    let mut s = AuctionSuite::new();
    s.create().unwrap();
    let mut bids = vec![];
    for i in 0..5 {
        let b = s.bidder(&format!("bid{i}"));
        s.commit(&b, 2, 2, 7).unwrap();
        bids.push(b);
    }
    let extra = s.bidder("extra");
    assert!(s.commit(&extra, 1, 0, 2).is_err());
    s.height(10);
    assert!(s.reveal(&bids[0], 2, 2).is_err());
    assert_eq!(s.auction().auction.total_demand, Uint256::zero());
    assert_eq!(s.auction().auction.total_collateral, Uint128::new(35));
}

#[test]
fn seller_controls_are_monotone_after_first_commit_and_sponsor_topup_does_not_resume() {
    // Break: trusting supplied actor/task, allowing cancellation, or lowering committed caps.
    let mut s = AuctionSuite::new();
    let mut request = s.request();
    request.gas = coin(101, "ugas");
    let funds = s.funds(&request);
    s.execute(
        s.seller.clone(),
        &ExecuteMsg::CreateAuction(request),
        &funds,
    )
    .unwrap();
    s.execute(
        s.seller.clone(),
        &ExecuteMsg::UpdateBudget {
            gas_per_poll: 200,
            max_gas_price: Decimal::one(),
        },
        &[],
    )
    .unwrap();
    // Host preflight pauses before guest execution, independent of the workflow.
    s.block(2).unwrap();
    let paused = s.funding().status;
    assert!(matches!(paused, TaskStatus::Paused { .. }));
    let available = s.funding().available;
    s.execute(
        s.sponsor.clone(),
        &ExecuteMsg::Async(AsyncExecuteMsg::FundTask { task_id: 1 }),
        &[coin(150, "ugas")],
    )
    .unwrap();
    assert_eq!(s.funding().available, available + Uint128::new(150));
    assert_eq!(s.funding().status, paused);
    assert!(s
        .execute(
            s.sponsor.clone(),
            &ExecuteMsg::Async(AsyncExecuteMsg::ResumeTask { task_id: 1 }),
            &[]
        )
        .is_err());
    assert!(s
        .execute(
            s.seller.clone(),
            &ExecuteMsg::Async(AsyncExecuteMsg::ResumeTask { task_id: 2 }),
            &[]
        )
        .is_err());
    s.execute(
        s.seller.clone(),
        &ExecuteMsg::UpdateBudget {
            gas_per_poll: 90,
            max_gas_price: Decimal::one(),
        },
        &[],
    )
    .unwrap();
    let b = s.bidder("bid");
    s.commit(&b, 2, 1, 20).unwrap();
    assert!(s
        .execute(
            s.seller.clone(),
            &ExecuteMsg::Async(AsyncExecuteMsg::CancelTask { task_id: 1 }),
            &[]
        )
        .is_err());
    for (gas, price) in [(89, Decimal::one()), (90, Decimal::percent(99))] {
        assert!(s
            .execute(
                s.seller.clone(),
                &ExecuteMsg::UpdateBudget {
                    gas_per_poll: gas,
                    max_gas_price: price
                },
                &[]
            )
            .is_err());
    }
    assert!(s
        .execute(
            s.sponsor.clone(),
            &ExecuteMsg::UpdateBudget {
                gas_per_poll: 100,
                max_gas_price: Decimal::one()
            },
            &[]
        )
        .is_err());
    // Serde-compatible extra fields have no authority: actual caller, saved task,
    // and immutable refund are derived by the contract.
    let injected = serde_json::json!({"update_budget":{"task_id":999,"actor":s.seller,"refund":s.sponsor,"gas_per_poll":100,"max_gas_price":"1"}});
    let msg: ExecuteMsg = serde_json::from_value(injected).unwrap();
    assert!(s.execute(s.sponsor.clone(), &msg, &[]).is_err());
    s.execute(s.seller.clone(), &msg, &[]).unwrap();
    assert_eq!(s.funding().refund, s.seller.to_string());
    assert_eq!(s.funding().gas_per_poll, 100);
    s.execute(
        s.seller.clone(),
        &ExecuteMsg::Async(AsyncExecuteMsg::ResumeTask { task_id: 1 }),
        &[],
    )
    .unwrap();
    assert_eq!(s.funding().status, TaskStatus::Runnable);
}

#[test]
fn empty_cancel_is_atomic_with_host_and_first_commit_wins_the_race() {
    // Break: business abort surviving a rejected host cancellation or a committed bid.
    let mut s = AuctionSuite::new();
    s.create().unwrap();
    assert!(s
        .execute(
            s.sponsor.clone(),
            &ExecuteMsg::Async(AsyncExecuteMsg::CancelTask { task_id: 1 }),
            &[]
        )
        .is_err());
    // Inject a host bank-refund failure; both host and business cancellation revert.
    s.reject_bank(&s.seller.clone(), "ugas");
    let before = s.auction().auction;
    assert!(s
        .execute(
            s.seller.clone(),
            &ExecuteMsg::Async(AsyncExecuteMsg::CancelTask { task_id: 1 }),
            &[]
        )
        .is_err());
    assert_eq!(s.auction().auction, before);
    assert_eq!(s.funding().status, TaskStatus::Runnable);
    s.allow_bank();
    s.execute(
        s.seller.clone(),
        &ExecuteMsg::Async(AsyncExecuteMsg::CancelTask { task_id: 1 }),
        &[],
    )
    .unwrap();
    assert_eq!(s.auction().auction.state, BusinessState::Aborted);
    let b = s.bidder("late");
    assert!(s.commit(&b, 1, 0, 2).is_err());
    assert!(s
        .execute(
            s.seller.clone(),
            &ExecuteMsg::Async(AsyncExecuteMsg::ResumeTask { task_id: 1 }),
            &[]
        )
        .is_err());
}

#[test]
fn every_nonfunding_public_route_rejects_attached_funds() {
    // Break: accepting donations through routes that should have no credit semantics.
    let mut s = AuctionSuite::new();
    s.create().unwrap();
    let b = s.bidder("bid");
    s.commit(&b, 2, 1, 20).unwrap();
    for msg in [
        ExecuteMsg::RevealBid(RevealBidRequest {
            quantity_lots: Uint128::new(2),
            price_tick: 1,
            beneficiary: s.beneficiary.to_string(),
            salt: Binary::from(vec![7; 32]),
        }),
        ExecuteMsg::Abort {},
        ExecuteMsg::ClaimSeller {},
        ExecuteMsg::ClaimBid {
            bidder: b.to_string(),
        },
        ExecuteMsg::UpdateBudget {
            gas_per_poll: 200,
            max_gas_price: Decimal::one(),
        },
        ExecuteMsg::Async(AsyncExecuteMsg::ResumeTask { task_id: 1 }),
        ExecuteMsg::Async(AsyncExecuteMsg::CancelTask { task_id: 1 }),
    ] {
        assert!(s
            .execute(s.seller.clone(), &msg, &[coin(1, "ugas")])
            .is_err());
    }
}

#[test]
fn pagination_is_exclusive_bounded_and_complete() {
    // Break: inclusive cursor, unbounded loading, wrong default, or omitted final bid.
    let mut s = AuctionSuite::new();
    let mut r = s.request();
    r.terms.max_bids = 256;
    r.terms.batch_size = 32;
    let f = s.funds(&r);
    s.execute(s.seller.clone(), &ExecuteMsg::CreateAuction(r), &f)
        .unwrap();
    for i in 0..256 {
        let b = s.bidder(&format!("page{i}"));
        s.commit(&b, 1, 0, 2).unwrap();
    }
    let page = |s: &AuctionSuite, start_after, limit| {
        s.app
            .wrap()
            .query_wasm_smart::<Vec<BidRecord>>(&s.contract, &QueryMsg::Bids { start_after, limit })
    };
    assert_eq!(page(&s, None, None).unwrap().len(), 16);
    assert_eq!(page(&s, None, Some(32)).unwrap().len(), 32);
    assert!(page(&s, None, Some(0)).is_err());
    assert!(page(&s, None, Some(33)).is_err());
    let mut seen = vec![];
    let mut cursor = None;
    loop {
        let p = page(&s, cursor, Some(32)).unwrap();
        if p.is_empty() {
            break;
        }
        cursor = p.last().map(|b| b.index);
        seen.extend(p.into_iter().map(|b| b.index));
    }
    assert_eq!(seen, (0..256).collect::<Vec<u16>>());
    assert!(page(&s, Some(u16::MAX), Some(32)).unwrap().is_empty());
}

#[test]
fn funding_deposits_and_all_raw_control_ids_are_narrowly_bound() {
    // Break: accepting extra gas coins or addressing another host task by raw ID.
    let mut s = AuctionSuite::new();
    s.create().unwrap();
    let before = s.funding();
    for funds in [
        vec![],
        vec![coin(1, "uquote")],
        vec![coin(1, "ugas"), coin(1, "uquote")],
    ] {
        assert!(s
            .execute(
                s.sponsor.clone(),
                &ExecuteMsg::Async(AsyncExecuteMsg::FundTask { task_id: 1 }),
                &funds
            )
            .is_err());
    }
    for msg in [
        AsyncExecuteMsg::FundTask { task_id: 2 },
        AsyncExecuteMsg::ResumeTask { task_id: 2 },
        AsyncExecuteMsg::CancelTask { task_id: 2 },
    ] {
        let funds = if matches!(msg, AsyncExecuteMsg::FundTask { .. }) {
            vec![coin(1, "ugas")]
        } else {
            vec![]
        };
        assert!(s
            .execute(s.seller.clone(), &ExecuteMsg::Async(msg), &funds)
            .is_err());
    }
    assert_eq!(s.funding(), before);
    assert!(s
        .app
        .wrap()
        .query_wasm_smart::<TaskFunding>(
            &s.contract,
            &QueryMsg::Async(AsyncQueryMsg::TaskFunding { task_id: 2 })
        )
        .is_err());
    let unknown = s.app.api().addr_make("not-a-bidder");
    assert_eq!(s.bid(&unknown), None);
    let config: FundingConfig = s
        .app
        .wrap()
        .query_wasm_smart(
            &s.contract,
            &QueryMsg::Async(AsyncQueryMsg::FundingConfig {}),
        )
        .unwrap();
    assert_eq!(config.denom, "ugas");
    assert!(serde_json::from_value::<ExecuteMsg>(
        serde_json::json!({"extend_lease":{"task_id":1,"allocated_bytes":100000,"expires_at":1000}})
    )
    .is_err());
}

#[test]
fn query_phases_follow_height_but_terminal_business_state_survives_host_pruning() {
    // Break: inferring finality/abort from a missing host task or scheduler progress.
    let mut s = AuctionSuite::new();
    s.create().unwrap();
    for (height, phase) in [
        (1, BiddingPhase::Commit),
        (10, BiddingPhase::Reveal),
        (20, BiddingPhase::Settlement),
        (60, BiddingPhase::AwaitingAbort),
    ] {
        s.height(height);
        assert_eq!(s.auction().phase, phase);
    }
    s.block(71).unwrap();
    assert!(matches!(
        s.auction().host,
        HostTaskView::Unavailable {
            lease_expired: true,
            ..
        }
    ));
    assert_eq!(s.auction().auction.state, BusinessState::Active);
    assert!(s
        .app
        .wrap()
        .query_wasm_smart::<TaskResponse>(&s.contract, &QueryMsg::Task {})
        .is_err());
    s.execute(s.sponsor.clone(), &ExecuteMsg::Abort {}, &[])
        .unwrap();
    assert_eq!(s.auction().phase, BiddingPhase::Aborted);
    {
        let mut storage = s.app.contract_storage_mut(&s.contract);
        let mut a = load_auction(&*storage).unwrap();
        a.state = BusinessState::Finalized;
        save_auction(&mut *storage, &a).unwrap();
    }
    assert_eq!(s.auction().phase, BiddingPhase::Finalized);
    assert!(matches!(s.auction().host, HostTaskView::Unavailable { .. }));
}

#[test]
fn auction_host_query_preserves_unboxed_json_representation() {
    // Break: adding a wire-level wrapper while reducing the Rust enum size.
    let mut s = AuctionSuite::new();
    s.create().unwrap();
    let auction: serde_json::Value = s
        .app
        .wrap()
        .query_wasm_smart(&s.contract, &QueryMsg::Auction {})
        .unwrap();
    let task: serde_json::Value = s
        .app
        .wrap()
        .query_wasm_smart(&s.contract, &QueryMsg::Task {})
        .unwrap();
    assert_eq!(auction["host"], serde_json::json!({"present": task}));
    // A consumer expecting the original unboxed response object still decodes.
    #[derive(serde::Deserialize)]
    struct OriginalPresent {
        present: TaskResponse,
    }
    let original: OriginalPresent = serde_json::from_value(auction["host"].clone()).unwrap();
    assert_eq!(original.present.task_id, 1);
    assert_eq!(original.present.program.kind, "native_batch_auction");
    let typed: AuctionView = serde_json::from_value(auction).unwrap();
    let HostTaskView::Present(present) = typed.host else {
        panic!("registered host task must be present");
    };
    assert_eq!(present.expires_at, 60);
}

type UnitDeps = OwnedDeps<
    testing::MockStorage,
    testing::MockApi,
    testing::MockQuerier<RuntimeQuery>,
    RuntimeQuery,
>;
fn unit(fee: &str) -> (UnitDeps, Env, MessageInfo, CreateAuctionRequest) {
    let mut deps = OwnedDeps {
        storage: testing::MockStorage::new(),
        api: testing::MockApi::default(),
        querier: testing::MockQuerier::new(&[]),
        custom_query_type: std::marker::PhantomData,
    };
    let fee = fee.to_string();
    deps.querier = deps.querier.with_custom_handler(move |q| match q {
        RuntimeQuery::Funding(SchedulerQuery::FundingConfig {}) => {
            SystemResult::Ok(ContractResult::Ok(
                to_json_binary(&FundingConfig {
                    denom: fee.clone(),
                    gas_price: Decimal::one(),
                    min_base_price: Decimal::one(),
                    target_gas: 500,
                    price_grace_blocks: 3,
                    last_block_poll_gas_used: 0,
                    max_gas_per_poll: 1000,
                    max_lifetime_blocks: 1000,
                })
                .unwrap(),
            ))
        }
        _ => SystemResult::Ok(ContractResult::Err("host unavailable".into())),
    });
    let mut env = testing::mock_env();
    env.block.height = 1;
    let info = MessageInfo {
        sender: deps.api.addr_make("unit-seller"),
        funds: vec![],
    };
    let fee_denom = fee_name(&deps);
    instantiate(
        deps.as_mut(),
        env.clone(),
        info.clone(),
        InstantiateMsg {
            sale_denom: "usale".into(),
            quote_denom: "uquote".into(),
            fee_denom,
        },
    )
    .unwrap();
    let mut r = AuctionSuite::new().request();
    r.gas.denom = fee_name(&deps);
    (deps, env, info, r)
}
fn fee_name(deps: &UnitDeps) -> String {
    let config: FundingConfig = deps
        .as_ref()
        .querier
        .query(&QueryRequest::Custom(RuntimeQuery::Funding(
            SchedulerQuery::FundingConfig {},
        )))
        .unwrap();
    config.denom
}
fn unit_create(
    deps: &mut UnitDeps,
    env: &Env,
    info: &MessageInfo,
    r: &CreateAuctionRequest,
) -> Response<RuntimeMsg> {
    let mut funds = std::collections::BTreeMap::from([(
        "usale".to_string(),
        r.terms.supply_lots * r.terms.lot_size,
    )]);
    *funds.entry(r.gas.denom.clone()).or_default() += r.gas.amount;
    execute(
        deps.as_mut(),
        env.clone(),
        MessageInfo {
            funds: funds
                .into_iter()
                .map(|(denom, amount)| Coin { denom, amount })
                .collect(),
            ..info.clone()
        },
        ExecuteMsg::CreateAuction(r.clone()),
    )
    .unwrap()
}
fn unit_reply(payload: Binary, id: u64) -> Reply {
    #[allow(deprecated)]
    Reply {
        id: 1,
        payload,
        gas_used: 0,
        result: SubMsgResult::Ok(SubMsgResponse {
            events: vec![],
            data: Some(to_json_binary(&id).unwrap()),
            msg_responses: vec![],
        }),
    }
}

#[test]
fn registration_reply_validates_full_host_identity_and_actual_task_id() {
    // Break: binding a fabricated response, wrong workflow, expiry, or reply tag.
    let (mut deps, env, info, r) = unit("ugas");
    let response = unit_create(&mut deps, &env, &info, &r);
    assert_eq!(response.messages[0].id, 1);
    assert_eq!(
        response.messages[0].payload,
        Binary::from(br#"{"auction_id":1}"#.as_slice())
    );
    let good = TaskResponse {
        contract: env.contract.address.to_string(),
        task_id: 41,
        program: TaskProgram {
            kind: "native_batch_auction".into(),
            version: 1,
            input: Binary::from(br#"{"auction_id":1}"#.as_slice()),
        },
        code_checksum: Binary::from(vec![3; 32]),
        status: TaskStatus::Runnable,
        wait_sequence: 0,
        state: None,
        wait: None,
        inbox: None,
        output: None,
        error: None,
        last_attempt: None,
        expires_at: 60,
        retain_until: 70,
        allocated_bytes: MIN_ALLOCATED_BYTES,
    };
    for mutation in 0..6 {
        let mut task = good.clone();
        match mutation {
            0 => task.contract = "wrong-owner".into(),
            1 => task.task_id = 42,
            2 => task.program.kind = "wrong".into(),
            3 => task.program.version = 2,
            4 => task.program.input = Binary::from(br#"{"auction_id":2}"#.as_slice()),
            _ => task.expires_at = 61,
        };
        deps.querier = deps.querier.with_custom_handler(move |_| {
            SystemResult::Ok(ContractResult::Ok(to_json_binary(&task).unwrap()))
        });
        assert!(reply(
            deps.as_mut(),
            env.clone(),
            unit_reply(response.messages[0].payload.clone(), 41)
        )
        .is_err());
        assert_eq!(load_auction(&deps.storage).unwrap().task_id, None);
    }
    deps.querier = deps.querier.with_custom_handler(move |_| {
        SystemResult::Ok(ContractResult::Ok(to_json_binary(&good).unwrap()))
    });
    for payload in [
        Binary::default(),
        Binary::from(b"bad".as_slice()),
        Binary::from(br#"{"auction_id":2}"#.as_slice()),
    ] {
        assert!(reply(deps.as_mut(), env.clone(), unit_reply(payload, 41)).is_err());
    }
    for fault in 0..5 {
        let mut rep = unit_reply(response.messages[0].payload.clone(), 41);
        #[allow(deprecated)]
        match fault {
            0 => rep.id = 2,
            1 => rep.result = SubMsgResult::Err("host reject".into()),
            2 => {
                if let SubMsgResult::Ok(ref mut v) = rep.result {
                    v.data = None
                }
            }
            3 => {
                if let SubMsgResult::Ok(ref mut v) = rep.result {
                    v.data = Some(Binary::from(b"bad".as_slice()))
                }
            }
            _ => rep = unit_reply(response.messages[0].payload.clone(), 0),
        };
        assert!(reply(deps.as_mut(), env.clone(), rep).is_err());
    }
    let result = reply(
        deps.as_mut(),
        env.clone(),
        unit_reply(response.messages[0].payload.clone(), 41),
    )
    .unwrap();
    assert_eq!(
        from_json::<CreateAuctionResponse>(result.data.unwrap()).unwrap(),
        CreateAuctionResponse {
            auction_id: 1,
            task_id: 41
        }
    );
    assert!(reply(
        deps.as_mut(),
        env,
        unit_reply(response.messages[0].payload.clone(), 41)
    )
    .is_err());
}

#[test]
fn instantiate_rejects_funds_invalid_denoms_and_host_mismatch() {
    // Break: accepting mislabeled fees, identical asset denominations, or funds at instantiate.
    let (mut deps, env, info, _) = unit("ugas");
    for (sale, quote, fee) in [
        ("usale", "usale", "ugas"),
        ("!sale", "uquote", "ugas"),
        ("usale", "!quote", "ugas"),
        ("usale", "uquote", "other"),
        ("usale", "uquote", ""),
    ] {
        assert!(instantiate(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            InstantiateMsg {
                sale_denom: sale.into(),
                quote_denom: quote.into(),
                fee_denom: fee.into()
            }
        )
        .is_err());
    }
    let mut info = info;
    info.funds = vec![coin(1, "ugas")];
    assert!(instantiate(
        deps.as_mut(),
        env,
        info,
        InstantiateMsg {
            sale_denom: "usale".into(),
            quote_denom: "uquote".into(),
            fee_denom: "ugas".into()
        }
    )
    .is_err());
}

#[test]
fn raw_duplicate_zero_and_coalescing_overflow_funds_are_rejected_without_writes() {
    // Break: SDK normalization hiding malformed coin vectors or wrapping a shared-denom sum.
    let (mut deps, env, info, r) = unit("ugas");
    for funds in [
        vec![coin(50, "usale"), coin(500, "ugas"), coin(500, "ugas")],
        vec![coin(50, "usale"), coin(1000, "ugas"), coin(0, "uquote")],
    ] {
        assert!(execute(
            deps.as_mut(),
            env.clone(),
            MessageInfo {
                funds,
                ..info.clone()
            },
            ExecuteMsg::CreateAuction(r.clone())
        )
        .is_err());
        assert!(deps.storage.get(AUCTION_KEY).is_none());
    }
    let (mut deps, env, info, mut r) = unit("usale");
    r.terms.supply_lots = Uint128::MAX;
    r.terms.lot_size = Uint128::one();
    r.terms.reserve_price = Uint128::one();
    r.terms.tick_count = 1;
    r.gas = coin(1, "usale");
    assert!(matches!(
        execute(deps.as_mut(), env, info, ExecuteMsg::CreateAuction(r)),
        Err(native_batch_auction::error::ContractError::Overflow(_))
    ));
    assert!(deps.storage.get(AUCTION_KEY).is_none());
}

#[test]
fn checked_collateral_and_demand_overflows_leave_bid_and_buckets_unchanged() {
    // Break: wrapping aggregate liabilities or a demand bucket before successful reveal.
    let (mut deps, mut env, info, r) = unit("ugas");
    unit_create(&mut deps, &env, &info, &r);
    let mut auction = load_auction(&deps.storage).unwrap();
    auction.total_collateral = Uint128::MAX;
    save_auction(&mut deps.storage, &auction).unwrap();
    let bidder = deps.api.addr_make("overflow-bid");
    assert!(execute(
        deps.as_mut(),
        env.clone(),
        MessageInfo {
            sender: bidder.clone(),
            funds: vec![coin(1, "uquote")]
        },
        ExecuteMsg::CommitBid {
            digest: Binary::from(vec![1; 32]),
            collateral: Uint128::one()
        }
    )
    .is_err());
    assert_eq!(load_auction(&deps.storage).unwrap(), auction);
    assert_eq!(bid_index(&deps.storage, &bidder).unwrap(), None);
    auction.total_collateral = Uint128::zero();
    save_auction(&mut deps.storage, &auction).unwrap();
    let beneficiary = deps.api.addr_make("overflow-beneficiary");
    let digest = commitment::commitment_digest(&commitment::CommitmentInput {
        chain_id: &env.block.chain_id,
        contract: &env.contract.address,
        bidder: &bidder,
        beneficiary: &beneficiary,
        auction_id: 1,
        collateral: Uint128::new(20),
        quantity_lots: Uint128::new(2),
        price_tick: 1,
        salt: &[7; 32],
    })
    .unwrap();
    execute(
        deps.as_mut(),
        env.clone(),
        MessageInfo {
            sender: bidder.clone(),
            funds: vec![coin(20, "uquote")],
        },
        ExecuteMsg::CommitBid {
            digest: digest.into(),
            collateral: Uint128::new(20),
        },
    )
    .unwrap();
    env.block.height = 10;
    for bucket in [true, false] {
        save_demand(
            &mut deps.storage,
            1,
            if bucket {
                Uint256::MAX
            } else {
                Uint256::zero()
            },
        )
        .unwrap();
        let mut a = load_auction(&deps.storage).unwrap();
        a.total_demand = if bucket {
            Uint256::zero()
        } else {
            Uint256::MAX
        };
        save_auction(&mut deps.storage, &a).unwrap();
        let before = deps
            .storage
            .range(None, None, Order::Ascending)
            .collect::<Vec<_>>();
        assert!(execute(
            deps.as_mut(),
            env.clone(),
            MessageInfo {
                sender: bidder.clone(),
                funds: vec![]
            },
            ExecuteMsg::RevealBid(RevealBidRequest {
                quantity_lots: Uint128::new(2),
                price_tick: 1,
                beneficiary: beneficiary.to_string(),
                salt: Binary::from(vec![7; 32])
            })
        )
        .is_err());
        assert_eq!(
            deps.storage
                .range(None, None, Order::Ascending)
                .collect::<Vec<_>>(),
            before
        );
    }
}

#[test]
fn host_outage_does_not_release_principal_or_replace_business_finality() {
    // Break: treating host query failure as permission for an early abort or refund.
    let (mut deps, mut env, info, r) = unit("ugas");
    unit_create(&mut deps, &env, &info, &r);
    let mut a = load_auction(&deps.storage).unwrap();
    a.task_id = Some(41);
    save_auction(&mut deps.storage, &a).unwrap();
    let view: AuctionView =
        from_json(query(deps.as_ref(), env.clone(), QueryMsg::Auction {}).unwrap()).unwrap();
    assert!(matches!(
        view.host,
        HostTaskView::Unavailable {
            lease_expired: false,
            ..
        }
    ));
    assert!(query(deps.as_ref(), env.clone(), QueryMsg::Task {})
        .unwrap_err()
        .to_string()
        .contains("host unavailable"));
    for msg in [ExecuteMsg::Abort {}, ExecuteMsg::ClaimSeller {}] {
        assert!(execute(deps.as_mut(), env.clone(), info.clone(), msg).is_err());
    }
    assert_eq!(load_auction(&deps.storage).unwrap(), a);
    for state in [BusinessState::Finalized, BusinessState::Aborted] {
        a.state = state.clone();
        save_auction(&mut deps.storage, &a).unwrap();
        for height in [19, 71] {
            env.block.height = height;
            let view: AuctionView =
                from_json(query(deps.as_ref(), env.clone(), QueryMsg::Auction {}).unwrap())
                    .unwrap();
            assert_eq!(view.auction.state, state);
            assert_eq!(
                view.phase,
                if state == BusinessState::Finalized {
                    BiddingPhase::Finalized
                } else {
                    BiddingPhase::Aborted
                }
            );
        }
    }
}
