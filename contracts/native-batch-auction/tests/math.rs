mod common;
use common::{oracle, terms};
use cosmwasm_std::{Uint128, Uint256};
use native_batch_auction::{
    math::{base_allocation, entitlement, price_at, validate_terms},
    state::Clearing,
};

#[test]
fn wide_demand_oracle_handles_aggregate_demand_above_uint128() {
    let mut t = terms();
    t.supply_lots = Uint128::new(170141183460469231731687303715884105727);
    t.lot_size = Uint128::one();
    t.reserve_price = Uint128::one();
    t.tick_count = 2;
    assert!(validate_terms(&t, 10).is_ok());
    let q = t.supply_lots.u128();
    let (clearing, allocations) = oracle(&t, &[Some((q, 1)), Some((q, 1)), Some((q, 1))]);
    assert_eq!(
        allocations,
        vec![
            56713727820156410577229101238628035243,
            56713727820156410577229101238628035242,
            56713727820156410577229101238628035242
        ]
    );
    assert_eq!(
        base_allocation(t.supply_lots, 1, &clearing).unwrap().u128(),
        56713727820156410577229101238628035242
    );
}

#[test]
fn admits_numeric_boundaries_and_rejects_zero_or_excess_caps() {
    let mut t = terms();
    t.tick_count = 1;
    t.max_bids = 1;
    t.batch_size = 1;
    assert!(validate_terms(&t, 10).is_ok());
    t.tick_count = 256;
    t.max_bids = 256;
    t.batch_size = 32;
    assert!(validate_terms(&t, 10).is_ok());
    for n in [0, 257] {
        let mut x = t.clone();
        x.tick_count = n;
        assert!(validate_terms(&x, 10).is_err());
        x = t.clone();
        x.max_bids = n;
        assert!(validate_terms(&x, 10).is_err());
    }
    for n in [0, 33] {
        let mut x = t.clone();
        x.batch_size = n;
        assert!(validate_terms(&x, 10).is_err());
    }
    for field in 0..4 {
        let mut x = t.clone();
        match field {
            0 => x.supply_lots = Uint128::zero(),
            1 => x.lot_size = Uint128::zero(),
            2 => x.reserve_price = Uint128::zero(),
            _ => x.tick_size = Uint128::zero(),
        };
        assert!(validate_terms(&x, 10).is_err());
    }
}

#[test]
fn checks_all_height_orderings_and_strict_worst_case_poll_margin() {
    let mut t = terms(); // ceil(3/2) + 2*ceil(5/2) + 4 = 12; R+12+1 < E.
    t.expires_at = 44;
    assert!(validate_terms(&t, 10).is_ok());
    t.expires_at = 43;
    assert!(validate_terms(&t, 10).is_err());
    for (c, r, e, creation) in [
        (20, 30, 100, 20),
        (20, 20, 100, 10),
        (30, 20, 100, 10),
        (20, 30, 30, 10),
        (20, u64::MAX - 1, u64::MAX, 10),
    ] {
        let mut x = terms();
        x.commit_end = c;
        x.reveal_end = r;
        x.expires_at = e;
        assert!(validate_terms(&x, creation).is_err());
    }
    let mut x = terms();
    x.tick_count = 1;
    x.max_bids = 1;
    x.batch_size = 32;
    x.expires_at = 39;
    assert!(validate_terms(&x, 10).is_ok());
    x.expires_at = 38;
    assert!(validate_terms(&x, 10).is_err());
}

#[test]
fn grid_prices_and_sale_and_quote_liabilities_must_fit_uint128() {
    let t = terms();
    assert_eq!(price_at(&t, 0).unwrap(), Uint128::new(2));
    assert_eq!(price_at(&t, 2).unwrap(), Uint128::new(4));
    assert!(price_at(&t, 3).is_err());
    let mut x = t.clone();
    x.supply_lots = Uint128::MAX;
    x.lot_size = Uint128::one();
    x.reserve_price = Uint128::one();
    x.tick_count = 1;
    assert!(validate_terms(&x, 10).is_ok());
    x.lot_size = Uint128::new(2);
    assert!(validate_terms(&x, 10).is_err());
    x.lot_size = Uint128::one();
    x.reserve_price = Uint128::new(2);
    assert!(validate_terms(&x, 10).is_err());
    x = t;
    x.reserve_price = Uint128::MAX;
    assert!(price_at(&x, 1).is_err());
    assert!(validate_terms(&x, 10).is_err());
    x.reserve_price = Uint128::one();
    x.tick_size = Uint128::MAX;
    assert!(price_at(&x, 2).is_err());
}

#[test]
fn underfull_and_exact_supply_pay_reserve_even_when_all_ticks_are_high() {
    for book in [
        vec![],
        vec![None],
        vec![Some((2, 2))],
        vec![Some((2, 2)), Some((3, 1))],
    ] {
        let (c, want) = oracle(&terms(), &book);
        assert_eq!(c.price, Uint128::new(2));
        assert_eq!(c.marginal_tick, None);
        for (i, bid) in book.iter().enumerate() {
            if let Some((q, t)) = bid {
                assert_eq!(
                    base_allocation(Uint128::new(*q), *t, &c).unwrap().u128(),
                    want[i]
                );
            }
        }
    }
}

#[test]
fn marginal_allocation_uses_wide_products_and_rejects_invalid_clearing() {
    let c = Clearing {
        price: Uint128::new(3),
        marginal_tick: Some(1),
        marginal_lots: Uint128::new(3),
        marginal_demand: Uint256::from(4u128),
    };
    assert_eq!(
        base_allocation(Uint128::new(2), 2, &c).unwrap(),
        Uint128::new(2)
    );
    assert_eq!(
        base_allocation(Uint128::new(2), 0, &c).unwrap(),
        Uint128::zero()
    );
    assert_eq!(
        base_allocation(Uint128::new(2), 1, &c).unwrap(),
        Uint128::one()
    );
    let huge = Clearing {
        marginal_lots: Uint128::MAX,
        marginal_demand: Uint256::from(Uint128::MAX),
        ..c.clone()
    };
    assert_eq!(
        base_allocation(Uint128::MAX, 1, &huge).unwrap(),
        Uint128::MAX
    );
    let invalid = Clearing {
        marginal_lots: Uint128::MAX,
        marginal_demand: Uint256::one(),
        ..c.clone()
    };
    assert!(base_allocation(Uint128::MAX, 1, &invalid).is_err());
    let zero = Clearing {
        marginal_demand: Uint256::zero(),
        ..c
    };
    assert!(base_allocation(Uint128::one(), 1, &zero).is_err());
}

#[test]
fn entitlement_rejects_payment_or_sale_overflow_and_insufficient_collateral() {
    assert_eq!(
        entitlement(
            Uint128::new(2),
            Uint128::new(8),
            Uint128::new(3),
            Uint128::new(10)
        )
        .unwrap()
        .sale,
        Uint128::new(20)
    );
    assert!(entitlement(
        Uint128::new(2),
        Uint128::one(),
        Uint128::one(),
        Uint128::one()
    )
    .is_err());
    assert!(entitlement(Uint128::MAX, Uint128::MAX, Uint128::new(2), Uint128::one()).is_err());
    assert!(entitlement(Uint128::MAX, Uint128::MAX, Uint128::one(), Uint128::new(2)).is_err());
    let none = entitlement(
        Uint128::zero(),
        Uint128::new(7),
        Uint128::new(3),
        Uint128::new(10),
    )
    .unwrap();
    assert_eq!(none.sale, Uint128::zero());
    assert_eq!(none.quote_refund, Uint128::new(7));
}

#[test]
fn standard_book_matches_independent_sorted_oracle_and_conserves_assets() {
    let book = [Some((2, 2)), Some((2, 1)), Some((2, 1)), Some((1, 0)), None];
    let deposits = [8, 6, 6, 2, 7];
    let (c, allocations) = oracle(&terms(), &book);
    assert_eq!(c.price, Uint128::new(3));
    assert_eq!(allocations, vec![2, 2, 1, 0, 0]);
    let base: Vec<_> = book
        .iter()
        .map(|bid| {
            bid.map(|(q, t)| base_allocation(Uint128::new(q), t, &c).unwrap().u128())
                .unwrap_or(0)
        })
        .collect();
    assert_eq!(base, vec![2, 1, 1, 0, 0]);
    let claims: Vec<_> = allocations
        .iter()
        .zip(deposits)
        .map(|(&a, d)| {
            entitlement(Uint128::new(a), Uint128::new(d), c.price, Uint128::new(10)).unwrap()
        })
        .collect();
    let refunds: Vec<_> = claims.iter().map(|e| e.quote_refund.u128()).collect();
    assert_eq!(refunds, vec![2, 0, 3, 2, 7]);
    let total_collateral: u128 = deposits.iter().sum();
    let seller_quote = total_collateral - refunds.iter().sum::<u128>();
    let seller_sale = 50 - claims.iter().map(|e| e.sale.u128()).sum::<u128>();
    assert_eq!((seller_quote, seller_sale), (15, 0));
    assert_eq!(total_collateral, 29);
}
