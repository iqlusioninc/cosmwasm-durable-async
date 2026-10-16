use crate::{
    error::ContractError,
    msg::AuctionTerms,
    state::{BidEntitlement, Clearing},
};
use cosmwasm_std::{Uint128, Uint256};

pub fn validate_terms(terms: &AuctionTerms, creation_height: u64) -> Result<(), ContractError> {
    if terms.supply_lots.is_zero()
        || terms.lot_size.is_zero()
        || terms.reserve_price.is_zero()
        || terms.tick_size.is_zero()
        || !(1..=256).contains(&terms.tick_count)
        || !(1..=256).contains(&terms.max_bids)
        || !(1..=32).contains(&terms.batch_size)
        || !(creation_height < terms.commit_end
            && terms.commit_end < terms.reveal_end
            && terms.reveal_end < terms.expires_at)
    {
        return Err(ContractError::InvalidTerms);
    }
    let batch = u64::from(terms.batch_size);
    // Bounded u16 counts make this sum representable; the absolute height is
    // still checked separately, including the strict final-height margin.
    let max_polls = u64::from(terms.tick_count).div_ceil(batch)
        + 2 * u64::from(terms.max_bids).div_ceil(batch)
        + 4;
    let latest = terms
        .reveal_end
        .checked_add(max_polls)
        .and_then(|height| height.checked_add(1))
        .ok_or(ContractError::InvalidTerms)?;
    if latest >= terms.expires_at {
        return Err(ContractError::InvalidTerms);
    }
    terms.supply_lots.checked_mul(terms.lot_size)?;
    terms
        .supply_lots
        .checked_mul(price_at(terms, terms.tick_count - 1)?)?;
    Ok(())
}
pub fn price_at(terms: &AuctionTerms, tick: u16) -> Result<Uint128, ContractError> {
    if tick >= terms.tick_count {
        return Err(ContractError::InvalidTick);
    }
    Ok(terms
        .reserve_price
        .checked_add(terms.tick_size.checked_mul(Uint128::from(tick))?)?)
}
/// Computes the floor allocation; the caller assigns marginal dust by commit
/// index. For n positive marginal bids, sum(q_i*M/Dm) = M, so the sum of the
/// fractional parts is an integer remainder strictly below n. If M < Dm each
/// floor is below q_i; if M = Dm there is no dust. One pass suffices.
pub fn base_allocation(
    quantity: Uint128,
    tick: u16,
    clearing: &Clearing,
) -> Result<Uint128, ContractError> {
    let Some(marginal) = clearing.marginal_tick else {
        return Ok(quantity);
    };
    match tick.cmp(&marginal) {
        std::cmp::Ordering::Greater => Ok(quantity),
        std::cmp::Ordering::Less => Ok(Uint128::zero()),
        std::cmp::Ordering::Equal => Ok(Uint256::from(quantity)
            .checked_mul(Uint256::from(clearing.marginal_lots))?
            .checked_div(clearing.marginal_demand)?
            .try_into()?),
    }
}
pub fn entitlement(
    allocation: Uint128,
    collateral: Uint128,
    price: Uint128,
    lot_size: Uint128,
) -> Result<BidEntitlement, ContractError> {
    let payment = allocation.checked_mul(price)?;
    Ok(BidEntitlement {
        sale: allocation.checked_mul(lot_size)?,
        quote_refund: collateral.checked_sub(payment)?,
    })
}
