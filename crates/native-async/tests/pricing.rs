use cosmwasm_std::Decimal;
use native_async::{bid_component, next_base_price};
use serde::Deserialize;
use std::str::FromStr;

#[derive(Deserialize)]
struct Fixture {
    prices: Vec<Price>,
    bid_components: Vec<Bid>,
}

#[derive(Deserialize)]
struct Price {
    name: String,
    base: Decimal,
    used: u64,
    target: u64,
    floor: Decimal,
    denominator: u64,
    next: Decimal,
}

#[derive(Deserialize)]
struct Bid {
    bid: Decimal,
    key: String,
}

// Catches wrong adjustment direction, rounding, floor/saturation, or key inversion.
#[test]
fn base_price_fixture_matches() {
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../../fixtures/native-async/v1/base-price.json"
    ))
    .unwrap();
    for price in fixture.prices {
        assert_eq!(
            next_base_price(
                price.base,
                price.used,
                price.target,
                price.floor,
                price.denominator,
            )
            .unwrap(),
            price.next,
            "{}",
            price.name,
        );
    }
    for bid in fixture.bid_components {
        assert_eq!(bid_component(bid.bid), bid.key, "bid {}", bid.bid);
    }
}

#[test]
fn zero_divisors_are_rejected() {
    assert!(next_base_price(Decimal::one(), 0, 0, Decimal::zero(), 8).is_err());
    assert!(next_base_price(Decimal::one(), 0, 1000, Decimal::zero(), 0).is_err());
}

// Catches narrow intermediate products or signed conversions of uint64 inputs.
#[test]
fn large_utilization_and_denominator_preserve_integer_atomics() {
    let base = Decimal::from_str("340282366920938463463.374607431768211455").unwrap();
    assert_eq!(
        next_base_price(base, u64::MAX, 1, Decimal::zero(), 1).unwrap(),
        base,
    );
    assert_eq!(
        next_base_price(Decimal::one(), u64::MAX, 1, Decimal::zero(), u64::MAX).unwrap(),
        Decimal::from_str("1.999999999999999999").unwrap(),
    );
    assert_eq!(
        next_base_price(Decimal::one(), 0, u64::MAX, Decimal::zero(), 1).unwrap(),
        Decimal::zero(),
    );
}
