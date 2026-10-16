#[allow(dead_code)]
mod common;
mod support;

use cosmwasm_std::*;
use native_async::*;
use native_batch_auction::{msg::*, state::*};
use support::AuctionSuite;

const BOOK: [Option<(u128, u16)>; 5] =
    [Some((2, 2)), Some((2, 1)), Some((2, 1)), Some((1, 0)), None];
const COLLATERAL: [u128; 5] = [8, 6, 6, 2, 7];

fn fixture(batch_size: u16) -> (AuctionSuite, Vec<Addr>) {
    let mut s = AuctionSuite::new();
    let mut request = s.request();
    request.terms.batch_size = batch_size;
    let funds = s.funds(&request);
    s.execute(
        s.seller.clone(),
        &ExecuteMsg::CreateAuction(request),
        &funds,
    )
    .unwrap();
    let bidders: Vec<_> = (0..BOOK.len())
        .map(|i| s.bidder(&format!("workflow-bidder-{i}")))
        .collect();
    for ((bidder, bid), collateral) in bidders.iter().zip(BOOK).zip(COLLATERAL) {
        let (quantity, tick) = bid.unwrap_or((1, 0));
        s.commit(bidder, quantity, tick, collateral).unwrap();
    }
    s.height(10);
    for (bidder, bid) in bidders.iter().zip(BOOK) {
        if let Some((quantity, tick)) = bid {
            s.reveal(bidder, quantity, tick).unwrap();
        }
    }
    (s, bidders)
}

#[test]
fn incremental_matches_oracle_for_every_batch_size() {
    // Break: wrong marginal tick, rounding, dust order, or skipped/repeated cursor item.
    for batch in 1..=32 {
        let (mut s, bidders) = fixture(batch);
        let (clearing, expected) = common::oracle(&s.auction().auction.terms, &BOOK);
        for h in 20..60 {
            s.block(h).unwrap();
            if s.auction().auction.state == BusinessState::Finalized {
                break;
            }
        }
        let auction = s.auction().auction;
        assert_eq!(auction.state, BusinessState::Finalized, "batch {batch}");
        assert_eq!(auction.settlement.clearing, Some(clearing));
        let actual: Vec<_> = bidders
            .iter()
            .map(|b| s.bid(b).unwrap().allocated_lots.u128())
            .collect();
        assert_eq!(actual, expected);
        assert_eq!(actual, vec![2, 2, 1, 0, 0]);
        assert_eq!(
            auction.settlement.clearing.as_ref().unwrap().price,
            Uint128::new(3)
        );
        assert_eq!(
            bidders
                .iter()
                .map(|b| s.bid(b).unwrap().entitlement.unwrap().quote_refund.u128())
                .collect::<Vec<_>>(),
            vec![2, 0, 3, 2, 7]
        );
        assert_eq!(auction.total_collateral, Uint128::new(29));
        assert_eq!(auction.settlement.bidder_sale_total, Uint128::new(50));
        assert_eq!(auction.settlement.seller_quote, Uint128::new(15));
        assert_eq!(auction.settlement.bidder_refund_total, Uint128::new(14));
    }
}

#[test]
fn one_phase_and_at_most_b_distinct_record_keys_per_poll() {
    // Break: processing the next phase in the same invocation or repeated same-height polls.
    let (mut s, _) = fixture(2);
    for h in 20..60 {
        let before = s.auction().auction.settlement;
        let report = s.block(h).unwrap();
        let trace = support::instrumented::take_trace();
        assert!(trace.distinct_record_keys.len() <= 2);
        assert_eq!(trace.ranges, 0);
        assert_eq!(trace.messages, 0);
        let after = s.auction().auction.settlement;
        assert_eq!(report.polls, 1);
        assert_eq!(s.block(h).unwrap().polls, 0);
        match before.phase {
            SettlementPhase::ScanPrice => assert!(matches!(
                after.phase,
                SettlementPhase::ScanPrice | SettlementPhase::AllocateBase
            )),
            SettlementPhase::AllocateBase => assert!(matches!(
                after.phase,
                SettlementPhase::AllocateBase | SettlementPhase::BuildClaims
            )),
            SettlementPhase::BuildClaims => assert!(matches!(
                after.phase,
                SettlementPhase::BuildClaims | SettlementPhase::Finalize
            )),
            SettlementPhase::Finalize => {
                assert_eq!(after.phase, SettlementPhase::Done);
                break;
            }
            SettlementPhase::Done => panic!("unexpected extra poll"),
        }
        if before.phase == after.phase {
            assert!(before.cursor.abs_diff(after.cursor) <= 2);
        }
    }
    assert_eq!(s.auction().auction.state, BusinessState::Finalized);
}

#[test]
fn claims_locked_during_every_phase() {
    // Break: allowing provisional entitlements or seller proceeds to escape before finalization.
    let (mut s, bidders) = fixture(1);
    let mut phases = Vec::new();
    for h in 20..60 {
        let phase = s.auction().auction.settlement.phase;
        if !phases.contains(&phase) {
            phases.push(phase);
        }
        for bidder in &bidders {
            assert!(s
                .execute(
                    s.sponsor.clone(),
                    &ExecuteMsg::ClaimBid {
                        bidder: bidder.to_string()
                    },
                    &[]
                )
                .is_err());
        }
        assert!(s
            .execute(s.sponsor.clone(), &ExecuteMsg::ClaimSeller {}, &[])
            .is_err());
        s.block(h).unwrap();
        if s.auction().auction.state == BusinessState::Finalized {
            break;
        }
    }
    assert_eq!(
        phases,
        vec![
            SettlementPhase::ScanPrice,
            SettlementPhase::AllocateBase,
            SettlementPhase::BuildClaims,
            SettlementPhase::Finalize
        ]
    );
    s.execute(
        s.sponsor.clone(),
        &ExecuteMsg::ClaimBid {
            bidder: bidders[0].to_string(),
        },
        &[],
    )
    .unwrap();
    assert_eq!(s.balance(&s.beneficiary, "usale"), Uint128::new(20));
}

#[test]
fn failed_batch_aborts_instead_of_committing_fail() {
    // Break: generated helper `?` committing a Fail after the first bid was written.
    let (mut s, bidders) = fixture(2);
    s.block(20).unwrap();
    assert_eq!(
        s.auction().auction.settlement.phase,
        SettlementPhase::AllocateBase
    );
    {
        let mut storage = s.app.contract_storage_mut(&s.contract);
        let mut bid = load_bid(&*storage, 1).unwrap();
        bid.allocated_lots = Uint128::one(); // impossible before this cursor is visited
        save_bid(&mut *storage, &bid).unwrap();
    }
    let auction_before = s.auction().auction;
    let bids_before: Vec<_> = bidders.iter().map(|b| s.bid(b).unwrap()).collect();
    let balances_before = [
        s.balance(&s.contract, "usale"),
        s.balance(&s.contract, "uquote"),
    ];
    let fees_before = s.funding().cumulative_execution_fees;
    s.block(21).unwrap();
    assert_eq!(s.auction().auction, auction_before);
    assert_eq!(
        bidders
            .iter()
            .map(|b| s.bid(b).unwrap())
            .collect::<Vec<_>>(),
        bids_before
    );
    assert_eq!(
        [
            s.balance(&s.contract, "usale"),
            s.balance(&s.contract, "uquote")
        ],
        balances_before
    );
    assert!(s.funding().cumulative_execution_fees > fees_before);
    assert!(matches!(s.funding().status, TaskStatus::Paused { .. }));
    assert_eq!(support::instrumented::take_trace().bid_writes, 1);
}

#[test]
fn prefix_faults_restore_business_checkpoint_and_balances_but_charge_gas() {
    // Break: retaining any part of an aborted batch, or losing the checkpoint on pause.
    use support::instrumented::{fault, take_trace, Fault};
    for phase in [SettlementPhase::AllocateBase, SettlementPhase::BuildClaims] {
        for (fault_kind, reason) in [
            (Fault::Helper, PauseReason::Trap),
            (Fault::Trap, PauseReason::Trap),
            (Fault::OutOfGas, PauseReason::OutOfGas),
        ] {
            let (mut s, bidders) = fixture(2);
            let mut fault_height = 20;
            while s.auction().auction.settlement.phase != phase {
                s.block(fault_height).unwrap();
                fault_height += 1;
            }
            let before = s.auction().auction;
            let before_bids: Vec<_> = bidders.iter().map(|b| s.bid(b).unwrap()).collect();
            let task_before = s.auction().host;
            let balances = [
                s.balance(&s.contract, "usale"),
                s.balance(&s.contract, "uquote"),
            ];
            let fees_before = s.funding().cumulative_execution_fees;
            let fees_account = s.app.api().addr_make("fees");
            let bank_fees_before = s.balance(&fees_account, "ugas");
            fault(
                &mut *s.app.contract_storage_mut(&s.contract),
                Some(fault_kind),
            );
            s.block(fault_height).unwrap();
            assert_eq!(
                take_trace().bid_writes,
                1,
                "fault must occur after a real nonempty prefix"
            );
            assert_eq!(s.auction().auction, before);
            assert_eq!(
                bidders
                    .iter()
                    .map(|b| s.bid(b).unwrap())
                    .collect::<Vec<_>>(),
                before_bids
            );
            assert_eq!(
                [
                    s.balance(&s.contract, "usale"),
                    s.balance(&s.contract, "uquote")
                ],
                balances
            );
            let fees_after = s.funding().cumulative_execution_fees;
            assert!(fees_after > fees_before);
            assert_eq!(
                s.balance(&fees_account, "ugas") - bank_fees_before,
                fees_after - fees_before
            );
            assert!(
                matches!(s.funding().status, TaskStatus::Paused { reason: actual, .. } if actual == reason)
            );
            let task_after = s.auction().host;
            match (task_before, task_after) {
                (
                    native_batch_auction::HostTaskView::Present(before),
                    native_batch_auction::HostTaskView::Present(after),
                ) => {
                    assert_eq!(before.state, after.state);
                    assert_eq!(before.wait_sequence, after.wait_sequence);
                }
                other => panic!("missing host task: {other:?}"),
            }
            fault(&mut *s.app.contract_storage_mut(&s.contract), None);
            s.reconstruct().unwrap();
            s.execute(
                s.seller.clone(),
                &ExecuteMsg::Async(AsyncExecuteMsg::ResumeTask {
                    task_id: before.task_id.unwrap(),
                }),
                &[],
            )
            .unwrap();
            for height in fault_height + 1..60 {
                s.block(height).unwrap();
                if s.auction().auction.state == BusinessState::Finalized {
                    break;
                }
            }
            assert_eq!(s.auction().auction.state, BusinessState::Finalized);
            assert_eq!(
                bidders
                    .iter()
                    .map(|b| s.bid(b).unwrap().allocated_lots.u128())
                    .collect::<Vec<_>>(),
                vec![2, 2, 1, 0, 0]
            );
            assert_eq!(s.balance(&s.contract, "usale"), Uint128::new(50));
        }
    }
}

#[test]
fn reconstructing_after_every_poll_matches_uninterrupted_result() {
    // Break: relying on Rust memory rather than the persisted phase and generated capture.
    for batch in [1, 2, 3, 32] {
        let (mut s, bidders) = fixture(batch);
        s.block(2).unwrap();
        s.reconstruct().unwrap();
        for height in 20..60 {
            s.block(height).unwrap();
            s.reconstruct().unwrap();
            if s.auction().auction.state == BusinessState::Finalized {
                break;
            }
        }
        assert_eq!(s.auction().auction.state, BusinessState::Finalized);
        assert_eq!(
            bidders
                .iter()
                .map(|b| s.bid(b).unwrap().allocated_lots.u128())
                .collect::<Vec<_>>(),
            vec![2, 2, 1, 0, 0]
        );
        assert_eq!(
            s.auction().auction.settlement.bidder_refund_total,
            Uint128::new(14)
        );
    }
}

#[test]
fn generated_checkpoint_and_poll_envelopes_remain_small() {
    // Break: capturing the whole bid book, or growing captures with the cursor.
    let (mut s, _) = fixture(1);
    let mut max_checkpoint = 0;
    let mut max_envelope = 0;
    for height in std::iter::once(2).chain(20..60) {
        s.block(height).unwrap();
        let trace = support::instrumented::take_trace();
        max_checkpoint = max_checkpoint.max(trace.checkpoint_bytes);
        max_envelope = max_envelope.max(trace.envelope_bytes);
        assert_eq!(trace.messages, 0);
        assert!(trace.distinct_record_keys.len() <= 1);
        if s.auction().auction.state == BusinessState::Finalized {
            break;
        }
    }
    // The largest capture is the initial positive sleep spanning 20 decimal digits;
    // settlement captures always sleep one block and the auction identity is fixed.
    let mut terms = common::terms();
    terms.reveal_end = u64::MAX - 100;
    terms.expires_at = u64::MAX;
    let mut storage = direct_book(terms, &[]);
    let api = cosmwasm_std::testing::MockApi::default();
    let querier = cosmwasm_std::testing::MockQuerier::<Empty>::new(&[]);
    let mut env = cosmwasm_std::testing::mock_env();
    env.block.height = 1;
    let response = native_batch_auction::sudo(
        DepsMut {
            storage: &mut storage,
            api: &api,
            querier: QuerierWrapper::new(&querier),
        },
        env,
        TaskSudoMsg::PollTask(PollRequest {
            task_id: 1,
            wait_sequence: 0,
            kind: "native_batch_auction".into(),
            version: 1,
            step: PollStep::Start {
                input: to_json_binary(&WorkflowInput { auction_id: 1 }).unwrap(),
            },
        }),
    )
    .unwrap();
    let data = response.data.unwrap();
    let poll: PollResponse = from_json(&data).unwrap();
    assert!(poll.messages.is_empty());
    match poll.outcome {
        PollOutcome::Pending {
            state,
            wait: Wait::Height { height },
        } => {
            assert_eq!(height, u64::MAX - 100);
            max_checkpoint = max_checkpoint.max(state.len());
            max_envelope = max_envelope.max(data.len());
        }
        other => panic!("expected positive long sleep: {other:?}"),
    }
    assert!(max_checkpoint > 0);
    assert!(max_checkpoint <= native_batch_auction::MAX_CHECKPOINT_BYTES);
    assert!(max_envelope <= 16384);
    println!("measured generated auction checkpoint={max_checkpoint} bytes poll envelope={max_envelope} bytes");
}

fn direct_book(
    terms: AuctionTerms,
    book: &[Option<(u128, u16)>],
) -> cosmwasm_std::testing::MockStorage {
    let mut suite = AuctionSuite::new();
    suite.create().unwrap();
    let mut a = suite.auction().auction;
    a.terms = terms;
    a.settlement.cursor = a.terms.tick_count;
    a.commit_count = book.len().try_into().unwrap();
    let mut storage = cosmwasm_std::testing::MockStorage::new();
    for (index, bid) in book.iter().enumerate() {
        let collateral = bid
            .map(|(q, t)| {
                Uint128::new(q) * (a.terms.reserve_price + a.terms.tick_size * Uint128::from(t))
            })
            .unwrap_or(Uint128::one());
        let reveal = bid.map(|(q, t)| RevealedBid {
            quantity_lots: Uint128::new(q),
            price_tick: t,
            beneficiary: suite.beneficiary.clone(),
        });
        a.total_collateral = a.total_collateral.checked_add(collateral).unwrap();
        if let Some((q, tick)) = bid {
            let quantity = Uint256::from(*q);
            a.total_demand = a.total_demand.checked_add(quantity).unwrap();
            let demand = load_demand(&storage, *tick)
                .unwrap()
                .checked_add(quantity)
                .unwrap();
            save_demand(&mut storage, *tick, demand).unwrap();
        }
        save_bid(
            &mut storage,
            &BidRecord {
                index: index.try_into().unwrap(),
                bidder: suite.app.api().addr_make(&format!("direct-bidder-{index}")),
                digest: Binary::from(vec![0; 32]),
                collateral,
                reveal,
                allocated_lots: Uint128::zero(),
                entitlement: None,
                claimed: false,
            },
        )
        .unwrap();
    }
    save_auction(&mut storage, &a).unwrap();
    storage
}

#[test]
fn independent_oracle_covers_empty_underfull_exact_and_tied_books_for_every_batch() {
    // Break: an empty phase, reserve-price boundary, tick-zero scan, or dust tie handled incorrectly.
    let books = [
        vec![],
        vec![None, None],
        vec![Some((2, 2))],
        vec![Some((2, 2)), Some((3, 1))],
        vec![Some((2, 0)), Some((2, 0)), Some((2, 0))],
        vec![Some((5, 2)), Some((5, 2)), Some((5, 2))],
        vec![Some((2, 2)), Some((3, 1)), Some((3, 1)), Some((2, 0)), None],
    ];
    for book in books {
        for batch in 1..=32 {
            let mut terms = common::terms();
            terms.batch_size = batch;
            let (clearing, expected) = common::oracle(&terms, &book);
            let mut storage = direct_book(terms, &book);
            for height in 30..100 {
                let progress =
                    native_batch_auction::settlement::advance_batch(&mut storage, height, 1)
                        .unwrap();
                if progress == native_batch_auction::settlement::BatchProgress::Finalized {
                    break;
                }
            }
            let a = load_auction(&storage).unwrap();
            assert_eq!(a.state, BusinessState::Finalized);
            assert_eq!(a.settlement.clearing, Some(clearing));
            assert_eq!(
                (0..a.commit_count)
                    .map(|i| load_bid(&storage, i).unwrap().allocated_lots.u128())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(
                a.settlement.bidder_refund_total + a.settlement.seller_quote,
                a.total_collateral
            );
        }
    }
}

#[test]
fn maximum_width_allocations_and_pro_rata_products_are_checked() {
    // Break: truncating a 256-bit numerator, using u128 products, or underflowing tick zero.
    for (supply, quantities) in [
        (u128::MAX, vec![u128::MAX]),
        (u128::MAX / 3, vec![u128::MAX / 3, u128::MAX / 3]),
    ] {
        let mut terms = common::terms();
        terms.supply_lots = Uint128::new(supply);
        terms.lot_size = Uint128::one();
        terms.reserve_price = Uint128::one();
        terms.tick_count = 1;
        terms.batch_size = 1;
        let book: Vec<_> = quantities.iter().map(|q| Some((*q, 0))).collect();
        let (_, expected) = common::oracle(&terms, &book);
        let mut storage = direct_book(terms, &book);
        for h in 30..100 {
            if native_batch_auction::settlement::advance_batch(&mut storage, h, 1).unwrap()
                == native_batch_auction::settlement::BatchProgress::Finalized
            {
                break;
            }
        }
        let a = load_auction(&storage).unwrap();
        assert_eq!(a.state, BusinessState::Finalized);
        assert_eq!(a.settlement.final_allocated, Uint128::new(supply));
        assert_eq!(
            (0..a.commit_count)
                .map(|i| load_bid(&storage, i).unwrap().allocated_lots.u128())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            a.settlement.seller_quote + a.settlement.bidder_refund_total,
            a.total_collateral
        );
    }
}

#[test]
fn helpers_reject_wrong_task_terminal_state_and_every_height_boundary_without_writes() {
    // Break: bypassing task correlation, frozen-input height, or strict E in a helper.
    let baseline = direct_book(common::terms(), &[Some((2, 1))]);
    let records: Vec<_> = baseline.range(None, None, Order::Ascending).collect();
    for (task, height, state) in [
        (2, 30, BusinessState::Active),
        (1, 29, BusinessState::Active),
        (1, 100, BusinessState::Active),
        (1, 101, BusinessState::Active),
        (1, 30, BusinessState::Finalized),
        (1, 30, BusinessState::Aborted),
    ] {
        let mut storage = cosmwasm_std::testing::MockStorage::new();
        for (k, v) in &records {
            storage.set(k, v);
        }
        let mut a = load_auction(&storage).unwrap();
        a.state = state;
        save_auction(&mut storage, &a).unwrap();
        let before: Vec<_> = storage.range(None, None, Order::Ascending).collect();
        assert!(
            native_batch_auction::settlement::advance_batch(&mut storage, height, task).is_err()
        );
        assert_eq!(
            storage
                .range(None, None, Order::Ascending)
                .collect::<Vec<_>>(),
            before
        );
    }
}

#[test]
fn helpers_reject_corrupted_immutable_terms_before_any_settlement_write() {
    // Break: trusting invalid persisted prices even though every poll is a new invocation.
    let mut storage = direct_book(common::terms(), &[]);
    let mut a = load_auction(&storage).unwrap();
    a.terms.reserve_price = Uint128::zero();
    save_auction(&mut storage, &a).unwrap();
    let before: Vec<_> = storage.range(None, None, Order::Ascending).collect();
    assert!(native_batch_auction::settlement::advance_batch(&mut storage, 30, 1).is_err());
    assert_eq!(
        storage
            .range(None, None, Order::Ascending)
            .collect::<Vec<_>>(),
        before
    );
}

#[test]
fn unknown_workflow_or_malformed_checkpoint_and_inconsistent_finalize_abort() {
    // Break: accepting an unknown ABI/capture or finalizing without aggregate conservation.
    let mut storage = direct_book(common::terms(), &[Some((2, 1))]);
    let original: Vec<_> = storage.range(None, None, Order::Ascending).collect();
    let api = cosmwasm_std::testing::MockApi::default();
    let querier = cosmwasm_std::testing::MockQuerier::<Empty>::new(&[]);
    let mut env = cosmwasm_std::testing::mock_env();
    env.block.height = 30;
    for (kind, version, wait_sequence, step) in [
        (
            "unknown",
            1,
            0,
            PollStep::Start {
                input: to_json_binary(&WorkflowInput { auction_id: 1 }).unwrap(),
            },
        ),
        (
            "native_batch_auction",
            2,
            0,
            PollStep::Start {
                input: to_json_binary(&WorkflowInput { auction_id: 1 }).unwrap(),
            },
        ),
        (
            "native_batch_auction",
            1,
            1,
            PollStep::Resume {
                state: Binary::from(b"invalid".as_slice()),
                resolution: Resolution::Timer {},
            },
        ),
        (
            "native_batch_auction",
            1,
            0,
            PollStep::Start {
                input: to_json_binary(&WorkflowInput { auction_id: 2 }).unwrap(),
            },
        ),
    ] {
        assert!(native_batch_auction::sudo(
            DepsMut {
                storage: &mut storage,
                api: &api,
                querier: QuerierWrapper::new(&querier)
            },
            env.clone(),
            TaskSudoMsg::PollTask(PollRequest {
                task_id: 1,
                wait_sequence,
                kind: kind.into(),
                version,
                step
            })
        )
        .is_err());
        assert_eq!(
            storage
                .range(None, None, Order::Ascending)
                .collect::<Vec<_>>(),
            original
        );
    }
    for height in 30..33 {
        native_batch_auction::settlement::advance_batch(&mut storage, height, 1).unwrap();
    }
    let mut a = load_auction(&storage).unwrap();
    assert_eq!(a.settlement.phase, SettlementPhase::Finalize);
    a.settlement.bidder_refund_total = Uint128::one();
    save_auction(&mut storage, &a).unwrap();
    assert!(native_batch_auction::settlement::advance_batch(&mut storage, 33, 1).is_err());
    assert_eq!(load_auction(&storage).unwrap().state, BusinessState::Active);
}
