mod support;

use cosmwasm_std::*;
use native_async::*;
use native_batch_auction::{msg::*, state::*, workflow::AuctionOutcome, HostTaskView};
use support::AuctionSuite;

#[test]
fn generated_wait_is_positive_and_initial_poll_at_reveal_starts_directly() {
    // Break: zero-height sleep or another pre-settlement wait after R.
    let mut s = AuctionSuite::new();
    s.create().unwrap();
    assert_eq!(s.block(2).unwrap().polls, 1);
    assert_eq!(s.block(19).unwrap().polls, 0);
    assert_eq!(s.block(20).unwrap().polls, 1);
    assert_eq!(
        s.auction().auction.settlement.phase,
        SettlementPhase::AllocateBase
    );
    assert_eq!(s.block(20).unwrap().polls, 0);
}

fn revealed(batch: u16) -> (AuctionSuite, Addr) {
    let mut s = AuctionSuite::new();
    let mut request = s.request();
    request.terms.batch_size = batch;
    let funds = s.funds(&request);
    s.execute(
        s.seller.clone(),
        &ExecuteMsg::CreateAuction(request),
        &funds,
    )
    .unwrap();
    let b = s.bidder("lifecycle-bidder");
    s.commit(&b, 2, 0, 20).unwrap();
    s.height(10);
    s.reveal(&b, 2, 0).unwrap();
    (s, b)
}

#[test]
fn finalize_at_e_minus_one_preserves_claims_through_expiry_and_pruning() {
    // Break: refusing the last valid finalization or deriving principal rights from retained tasks.
    let (mut s, b) = revealed(1);
    for height in 20..23 {
        s.block(height).unwrap();
    }
    assert_eq!(
        s.auction().auction.settlement.phase,
        SettlementPhase::Finalize
    );
    s.reconstruct().unwrap();
    s.block(59).unwrap();
    assert_eq!(s.auction().auction.state, BusinessState::Finalized);
    match s.auction().host {
        HostTaskView::Present(task) => assert_eq!(
            from_json::<AuctionOutcome>(task.output.unwrap()).unwrap(),
            AuctionOutcome::Finalized
        ),
        other => panic!("missing completed task: {other:?}"),
    }
    for height in [60, 70, 71, 72] {
        s.block(height).unwrap();
    }
    s.reconstruct().unwrap();
    assert!(matches!(s.auction().host, HostTaskView::Unavailable { .. }));
    assert!(s
        .execute(s.sponsor.clone(), &ExecuteMsg::Abort {}, &[])
        .is_err());
    s.execute(
        s.sponsor.clone(),
        &ExecuteMsg::ClaimBid {
            bidder: b.to_string(),
        },
        &[],
    )
    .unwrap();
    s.execute(b.clone(), &ExecuteMsg::ClaimSeller {}, &[])
        .unwrap();
    assert_eq!(s.balance(&s.beneficiary, "usale"), Uint128::new(20));
    assert_eq!(s.balance(&s.contract, "usale"), Uint128::zero());
    assert_eq!(s.balance(&s.contract, "uquote"), Uint128::zero());
    assert_eq!(s.auction().auction.state, BusinessState::Finalized);
}

#[test]
fn e_minus_one_unfinished_completes_without_an_expiry_timer_then_aborts() {
    // Break: scheduling a timer at E, unlocking provisional claims, or finalizing at E.
    let (mut s, b) = revealed(1);
    s.block(59).unwrap();
    assert_eq!(s.auction().auction.state, BusinessState::Active);
    assert_eq!(
        s.auction().auction.settlement.phase,
        SettlementPhase::AllocateBase
    );
    match s.auction().host {
        HostTaskView::Present(task) => {
            assert_eq!(
                from_json::<AuctionOutcome>(task.output.unwrap()).unwrap(),
                AuctionOutcome::LeaseWindowExhausted
            );
            assert!(task.wait.is_none());
        }
        other => panic!("missing completed task: {other:?}"),
    }
    assert!(s
        .execute(b.clone(), &ExecuteMsg::ClaimSeller {}, &[])
        .is_err());
    s.block(60).unwrap();
    s.execute(s.sponsor.clone(), &ExecuteMsg::Abort {}, &[])
        .unwrap();
    s.execute(
        s.sponsor.clone(),
        &ExecuteMsg::ClaimBid {
            bidder: b.to_string(),
        },
        &[],
    )
    .unwrap();
    assert_eq!(s.balance(&b, "uquote"), Uint128::new(1_000_000));
    assert_eq!(s.balance(&s.beneficiary, "usale"), Uint128::zero());
}

#[test]
fn expiry_in_every_partial_phase_refunds_full_principal() {
    // Break: ignoring deadline abort during one particular provisional phase.
    for polls in 0..=3 {
        let (mut s, b) = revealed(1);
        for height in 20..20 + polls {
            s.block(height).unwrap();
        }
        assert_eq!(s.auction().auction.state, BusinessState::Active);
        let progress = s.auction().auction.settlement;
        assert_eq!(s.block(60).unwrap().polls, 0);
        assert_eq!(s.auction().auction.settlement, progress);
        s.execute(
            s.sponsor.clone(),
            &ExecuteMsg::ClaimBid {
                bidder: b.to_string(),
            },
            &[],
        )
        .unwrap();
        s.execute(s.sponsor.clone(), &ExecuteMsg::ClaimSeller {}, &[])
            .unwrap();
        assert_eq!(s.balance(&b, "uquote"), Uint128::new(1_000_000));
        assert_eq!(s.balance(&s.contract, "usale"), Uint128::zero());
        assert_eq!(s.balance(&s.contract, "uquote"), Uint128::zero());
        assert_eq!(s.auction().auction.state, BusinessState::Aborted);
    }
}

#[test]
fn funding_a_paused_task_does_not_resume_settlement() {
    // Break: allowing funding to execute settlement or silently resume a gas pause.
    let (mut s, _) = revealed(1);
    s.execute(
        s.seller.clone(),
        &ExecuteMsg::UpdateBudget {
            gas_per_poll: 101,
            max_gas_price: Decimal::one(),
        },
        &[],
    )
    .unwrap();
    s.app
        .init_modules(|router, _, _| router.custom.runtime.config.host_gas = 100);
    s.block(20).unwrap();
    let progress = s.auction().auction.settlement;
    let paused = s.funding().status;
    assert!(matches!(
        paused,
        TaskStatus::Paused {
            reason: PauseReason::OutOfGas,
            ..
        }
    ));
    let task_id = s.auction().auction.task_id.unwrap();
    s.execute(
        s.sponsor.clone(),
        &ExecuteMsg::Async(AsyncExecuteMsg::FundTask { task_id }),
        &[coin(200, "ugas")],
    )
    .unwrap();
    assert_eq!(s.funding().status, paused);
    assert_eq!(s.block(21).unwrap().polls, 0);
    assert_eq!(s.auction().auction.settlement, progress);
    s.app
        .init_modules(|router, _, _| router.custom.runtime.config.host_gas = 2);
    s.execute(
        s.seller.clone(),
        &ExecuteMsg::Async(AsyncExecuteMsg::ResumeTask { task_id }),
        &[],
    )
    .unwrap();
    assert_eq!(s.block(22).unwrap().polls, 1);
    assert_eq!(
        s.auction().auction.settlement.phase,
        SettlementPhase::AllocateBase
    );
}

#[test]
fn abort_with_all_256_bids_performs_one_write_and_visits_no_bid_items() {
    // Break: sweeping the full book to erase provisional state or create refunds on abort.
    let mut s = AuctionSuite::new();
    let mut request = s.request();
    request.terms.max_bids = 256;
    request.terms.batch_size = 32;
    let funds = s.funds(&request);
    s.execute(
        s.seller.clone(),
        &ExecuteMsg::CreateAuction(request),
        &funds,
    )
    .unwrap();
    for index in 0..256 {
        let bidder = s.bidder(&format!("full-book-bidder-{index}"));
        s.commit(&bidder, 1, 0, 2).unwrap();
    }
    s.block(20).unwrap();
    s.block(21).unwrap();
    assert_eq!(s.auction().auction.commit_count, 256);
    s.height(60);
    s.execute(s.sponsor.clone(), &ExecuteMsg::Abort {}, &[])
        .unwrap();
    let trace = support::instrumented::take_trace();
    assert!(trace.distinct_record_keys.is_empty());
    assert_eq!(trace.ranges, 0);
    assert_eq!(trace.writes, 1);
    assert_eq!(s.auction().auction.state, BusinessState::Aborted);
    assert_eq!(s.auction().auction.total_collateral, Uint128::new(512));
}

#[test]
fn full_book_settlement_visits_every_bid_twice_and_finalizes_without_a_scan() {
    // Break: reloading prior batches, an unbounded finalizer, or allocating residual twice.
    let mut s = AuctionSuite::new();
    let mut request = s.request();
    request.terms.max_bids = 256;
    request.terms.batch_size = 32;
    let funds = s.funds(&request);
    s.execute(
        s.seller.clone(),
        &ExecuteMsg::CreateAuction(request),
        &funds,
    )
    .unwrap();
    let bidders: Vec<_> = (0..256)
        .map(|index| s.bidder(&format!("settle-full-book-{index}")))
        .collect();
    for bidder in &bidders {
        s.commit(bidder, 1, 2, 4).unwrap();
    }
    s.height(10);
    for bidder in &bidders {
        s.reveal(bidder, 1, 2).unwrap();
    }
    let mut total_items = 0;
    for height in 20..60 {
        let phase = s.auction().auction.settlement.phase;
        s.block(height).unwrap();
        let trace = support::instrumented::take_trace();
        assert!(trace.distinct_record_keys.len() <= 32);
        assert_eq!(trace.ranges, 0);
        assert_eq!(trace.messages, 0);
        if phase == SettlementPhase::Finalize {
            assert!(trace.distinct_record_keys.is_empty());
        }
        total_items += trace.distinct_record_keys.len();
        if s.auction().auction.state == BusinessState::Finalized {
            break;
        }
    }
    assert_eq!(total_items, 1 + 2 * 256);
    assert_eq!(s.auction().auction.state, BusinessState::Finalized);
    assert_eq!(
        s.auction().auction.settlement.seller_quote,
        Uint128::new(20)
    );
    for (index, bidder) in bidders.iter().enumerate() {
        assert_eq!(
            s.bid(bidder).unwrap().allocated_lots,
            Uint128::from(u8::from(index < 5))
        );
    }
}
