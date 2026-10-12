use cosmwasm_std::{Decimal, StdError, StdResult, Uint128, Uint256};

/// Adjust the block base price using truncating integer atomics, saturating at
/// the Decimal domain's maximum. Both divisors must be nonzero.
pub fn next_base_price(
    base: Decimal,
    used: u64,
    target: u64,
    floor: Decimal,
    denominator: u64,
) -> StdResult<Decimal> {
    if target == 0 || denominator == 0 {
        return Err(StdError::generic_err(
            "price adjustment divisors must be nonzero",
        ));
    }
    let base_atomics = Uint256::from(base.atomics());
    // A u128 price times a u64 utilization delta fits in Uint256.
    let magnitude = base_atomics * Uint256::from(used.abs_diff(target))
        / Uint256::from(target)
        / Uint256::from(denominator);
    let next = if used > target {
        (base_atomics + magnitude).min(Uint256::from(Uint128::MAX))
    } else {
        // Here the delta is at most target, so magnitude cannot exceed base.
        (base_atomics - magnitude).max(Uint256::from(floor.atomics()))
    };
    let atomics = Uint128::try_from(next).expect("price is bounded by u128 atomics");
    Ok(Decimal::new(atomics))
}

/// Fixed-width inverted atomics sort higher bids first in ascending key order.
pub fn bid_component(bid: Decimal) -> String {
    format!("{:032x}", u128::MAX - bid.atomics().u128())
}
