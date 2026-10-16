use cosmwasm_std::{
    testing::MockStorage, to_json_vec, Addr, Binary, Decimal, Order, Storage, Uint128, Uint256,
};
use native_batch_auction::{
    msg::{AuctionTerms, ExecuteMsg},
    state::*,
};

fn bid(index: u16, bidder: &str) -> BidRecord {
    BidRecord {
        index,
        bidder: Addr::unchecked(bidder),
        digest: Binary::from([0u8; 32]),
        collateral: Uint128::new(9),
        reveal: None,
        allocated_lots: Uint128::zero(),
        entitlement: None,
        claimed: false,
    }
}

#[test]
fn bid_updates_preserve_unique_index_binding_and_failed_writes_leave_state_intact() {
    let mut s = MockStorage::new();
    assert_eq!(bid_index(&s, &Addr::unchecked("a")).unwrap(), None);
    let mut a = bid(255, "a");
    save_bid(&mut s, &a).unwrap();
    assert_eq!(bid_index(&s, &a.bidder).unwrap(), Some(255));
    a.claimed = true;
    save_bid(&mut s, &a).unwrap();
    assert_eq!(load_bid(&s, 255).unwrap(), a);
    let before: Vec<_> = s.range(None, None, Order::Ascending).collect();
    assert!(save_bid(&mut s, &bid(256, "a")).is_err());
    assert!(save_bid(&mut s, &bid(255, "b")).is_err());
    assert_eq!(
        s.range(None, None, Order::Ascending).collect::<Vec<_>>(),
        before
    );
    save_bid(&mut s, &bid(256, "b")).unwrap();
    let indices: Vec<_> = s
        .range(
            Some(b"auction/v1/bid/"),
            Some(b"auction/v1/bid0"),
            Order::Ascending,
        )
        .map(|(k, _)| u16::from_be_bytes(k[k.len() - 2..].try_into().unwrap()))
        .collect();
    assert_eq!(indices, vec![255, 256]);
}

#[test]
fn bidder_lookup_rejects_malformed_or_mismatched_index_records() {
    let mut s = MockStorage::new();
    let a = bid(0, "a");
    save_bid(&mut s, &a).unwrap();
    s.set(b"auction/v1/bidder/a", &[0]);
    assert!(bid_index(&s, &a.bidder).is_err());
    s.set(b"auction/v1/bidder/a", &1u16.to_be_bytes());
    assert!(bid_index(&s, &a.bidder).is_err());
    save_bid(&mut s, &bid(1, "b")).unwrap();
    assert!(bid_index(&s, &a.bidder).is_err());
}

#[test]
fn tick_demand_is_separate_and_missing_ticks_are_zero() {
    let mut s = MockStorage::new();
    assert_eq!(load_demand(&s, 255).unwrap(), Uint256::zero());
    save_demand(&mut s, 255, Uint256::MAX).unwrap();
    save_demand(&mut s, 256, Uint256::one()).unwrap();
    save_bid(&mut s, &bid(255, "a")).unwrap();
    assert_eq!(load_demand(&s, 255).unwrap(), Uint256::MAX);
    assert_eq!(load_demand(&s, 256).unwrap(), Uint256::one());
}

#[test]
fn persisted_protocol_roundtrips_without_losing_wide_amounts() {
    let mut s = MockStorage::new();
    let config = Config {
        seller: Addr::unchecked("seller"),
        sale_denom: "sale".into(),
        quote_denom: "quote".into(),
        fee_denom: "fee".into(),
    };
    s.set(CONFIG_KEY, &to_json_vec(&config).unwrap());
    assert_eq!(load_config(&s).unwrap(), config);
    let auction = AuctionRecord {
        auction_id: 1,
        task_id: Some(2),
        created_at: 10,
        terms: AuctionTerms {
            supply_lots: Uint128::MAX,
            lot_size: Uint128::one(),
            reserve_price: Uint128::one(),
            tick_size: Uint128::one(),
            tick_count: 1,
            max_bids: 1,
            batch_size: 1,
            commit_end: 20,
            reveal_end: 30,
            expires_at: 100,
        },
        state: BusinessState::Active,
        commit_count: 0,
        total_collateral: Uint128::MAX,
        total_demand: Uint256::MAX,
        settlement: SettlementProgress {
            phase: SettlementPhase::ScanPrice,
            cursor: 0,
            higher_demand: Uint256::zero(),
            clearing: None,
            base_allocated: Uint128::zero(),
            residual: Uint128::zero(),
            final_allocated: Uint128::zero(),
            bidder_sale_total: Uint128::zero(),
            bidder_refund_total: Uint128::zero(),
            seller_quote: Uint128::zero(),
        },
        claimed: ClaimTotals {
            bidder_sale: Uint128::zero(),
            bidder_quote: Uint128::zero(),
            seller_sale: Uint128::zero(),
            seller_quote: Uint128::zero(),
        },
        seller_claimed: false,
        budget: Budget {
            gas_per_poll: 100,
            max_gas_price: Decimal::one(),
        },
    };
    save_auction(&mut s, &auction).unwrap();
    assert_eq!(load_auction(&s).unwrap(), auction);
    let msg = ExecuteMsg::CommitBid {
        digest: Binary::from([0u8; 32]),
        collateral: Uint128::MAX,
    };
    let json = serde_json::to_value(msg).unwrap();
    assert_eq!(json["commit_bid"]["collateral"], Uint128::MAX.to_string());
    assert_eq!(
        serde_json::to_value(SettlementPhase::BuildClaims).unwrap(),
        "build_claims"
    );
}
