//! One phase per invocation. The native poll transaction owns rollback.
use crate::{error::ContractError, math, state::*};
use cosmwasm_std::{StdError, Storage, Uint128, Uint256};

#[derive(Clone, Debug, PartialEq)]
pub enum BatchProgress {
    Continue,
    Finalized,
    LeaseWindowExhausted,
}

fn require(condition: bool) -> Result<(), ContractError> {
    if condition {
        Ok(())
    } else {
        Err(StdError::generic_err("inconsistent auction settlement").into())
    }
}

fn target(auction: &AuctionRecord) -> Result<Uint128, ContractError> {
    Ok(auction
        .total_demand
        .min(Uint256::from(auction.terms.supply_lots))
        .try_into()?)
}

pub fn advance_batch(
    storage: &mut dyn Storage,
    height: u64,
    task_id: u64,
) -> Result<BatchProgress, ContractError> {
    let mut a = load_auction(storage)?;
    if a.task_id != Some(task_id) {
        return Err(ContractError::InvalidTask);
    }
    if a.state != BusinessState::Active
        || height < a.terms.reveal_end
        || height >= a.terms.expires_at
    {
        return Err(ContractError::InvalidPhase);
    }
    math::validate_terms(&a.terms, a.created_at)?;
    require(a.commit_count <= a.terms.max_bids)?;
    match a.settlement.phase {
        SettlementPhase::ScanPrice => {
            require(
                a.settlement.clearing.is_none()
                    && a.settlement.cursor > 0
                    && a.settlement.cursor <= a.terms.tick_count
                    && a.settlement.higher_demand < Uint256::from(a.terms.supply_lots)
                    && a.settlement.base_allocated.is_zero()
                    && a.settlement.residual.is_zero()
                    && a.settlement.final_allocated.is_zero()
                    && a.settlement.bidder_sale_total.is_zero()
                    && a.settlement.bidder_refund_total.is_zero()
                    && a.settlement.seller_quote.is_zero(),
            )?;
            if a.total_demand <= Uint256::from(a.terms.supply_lots) {
                require(
                    a.settlement.cursor == a.terms.tick_count
                        && a.settlement.higher_demand.is_zero(),
                )?;
                a.settlement.clearing = Some(Clearing {
                    price: a.terms.reserve_price,
                    marginal_tick: None,
                    marginal_lots: Uint128::zero(),
                    marginal_demand: Uint256::zero(),
                });
            } else {
                for _ in 0..a.terms.batch_size {
                    // Exclusive remaining-tick bound, so tick zero never underflows.
                    let tick = a
                        .settlement
                        .cursor
                        .checked_sub(1)
                        .ok_or_else(|| StdError::generic_err("missing marginal tick"))?;
                    let demand = load_demand(storage, tick)?;
                    let through = a.settlement.higher_demand.checked_add(demand)?;
                    require(through <= a.total_demand)?;
                    a.settlement.cursor = tick;
                    if through >= Uint256::from(a.terms.supply_lots) {
                        a.settlement.clearing = Some(Clearing {
                            price: math::price_at(&a.terms, tick)?,
                            marginal_tick: Some(tick),
                            marginal_lots: a
                                .terms
                                .supply_lots
                                .checked_sub(a.settlement.higher_demand.try_into()?)?,
                            marginal_demand: demand,
                        });
                        break;
                    }
                    a.settlement.higher_demand = through;
                    require(tick > 0)?;
                }
            }
            if a.settlement.clearing.is_some() {
                a.settlement.phase = SettlementPhase::AllocateBase;
                a.settlement.cursor = 0;
            }
        }
        SettlementPhase::AllocateBase => {
            let clearing = a
                .settlement
                .clearing
                .as_ref()
                .ok_or_else(|| StdError::generic_err("missing clearing"))?;
            validate_clearing(&a, clearing)?;
            require(
                a.settlement.cursor <= a.commit_count
                    && a.settlement.residual.is_zero()
                    && a.settlement.final_allocated.is_zero()
                    && a.settlement.bidder_sale_total.is_zero()
                    && a.settlement.bidder_refund_total.is_zero()
                    && a.settlement.seller_quote.is_zero(),
            )?;
            let end = a.commit_count.min(
                a.settlement
                    .cursor
                    .checked_add(a.terms.batch_size)
                    .ok_or_else(|| StdError::generic_err("cursor overflow"))?,
            );
            for index in a.settlement.cursor..end {
                let mut bid = load_bid(storage, index)?;
                require(!bid.claimed && bid.entitlement.is_none() && bid.allocated_lots.is_zero())?;
                let allocated = match &bid.reveal {
                    Some(reveal) => {
                        validate_reveal(&a, &bid, reveal)?;
                        math::base_allocation(reveal.quantity_lots, reveal.price_tick, clearing)?
                    }
                    None => Uint128::zero(),
                };
                a.settlement.base_allocated = a.settlement.base_allocated.checked_add(allocated)?;
                require(a.settlement.base_allocated <= target(&a)?)?;
                bid.allocated_lots = allocated;
                save_bid(storage, &bid)?;
            }
            a.settlement.cursor = end;
            if end == a.commit_count {
                a.settlement.residual = target(&a)?.checked_sub(a.settlement.base_allocated)?;
                // The sum of fractional parts is less than the number of marginal bids.
                require(
                    a.settlement.residual.is_zero()
                        || a.settlement.residual < Uint128::from(a.commit_count),
                )?;
                a.settlement.phase = SettlementPhase::BuildClaims;
                a.settlement.cursor = 0;
            }
        }
        SettlementPhase::BuildClaims => {
            let clearing = a
                .settlement
                .clearing
                .as_ref()
                .ok_or_else(|| StdError::generic_err("missing clearing"))?;
            validate_clearing(&a, clearing)?;
            require(
                a.settlement.cursor <= a.commit_count
                    && a.settlement.base_allocated <= target(&a)?
                    && a.settlement.residual
                        <= target(&a)?.checked_sub(a.settlement.base_allocated)?,
            )?;
            let end = a.commit_count.min(
                a.settlement
                    .cursor
                    .checked_add(a.terms.batch_size)
                    .ok_or_else(|| StdError::generic_err("cursor overflow"))?,
            );
            for index in a.settlement.cursor..end {
                let mut bid = load_bid(storage, index)?;
                require(!bid.claimed && bid.entitlement.is_none())?;
                if let Some(reveal) = &bid.reveal {
                    validate_reveal(&a, &bid, reveal)?;
                    require(
                        bid.allocated_lots
                            == math::base_allocation(
                                reveal.quantity_lots,
                                reveal.price_tick,
                                clearing,
                            )?,
                    )?;
                    if !a.settlement.residual.is_zero()
                        && clearing.marginal_tick == Some(reveal.price_tick)
                        && bid.allocated_lots < reveal.quantity_lots
                    {
                        bid.allocated_lots = bid.allocated_lots.checked_add(Uint128::one())?;
                        a.settlement.residual = a.settlement.residual.checked_sub(Uint128::one())?;
                    }
                    require(bid.allocated_lots <= reveal.quantity_lots)?;
                } else {
                    require(bid.allocated_lots.is_zero())?;
                }
                let entitlement = math::entitlement(
                    bid.allocated_lots,
                    bid.collateral,
                    clearing.price,
                    a.terms.lot_size,
                )?;
                a.settlement.final_allocated = a
                    .settlement
                    .final_allocated
                    .checked_add(bid.allocated_lots)?;
                require(a.settlement.final_allocated <= target(&a)?)?;
                a.settlement.bidder_sale_total = a
                    .settlement
                    .bidder_sale_total
                    .checked_add(entitlement.sale)?;
                a.settlement.bidder_refund_total = a
                    .settlement
                    .bidder_refund_total
                    .checked_add(entitlement.quote_refund)?;
                a.settlement.seller_quote = a
                    .settlement
                    .seller_quote
                    .checked_add(bid.collateral.checked_sub(entitlement.quote_refund)?)?;
                bid.entitlement = Some(entitlement);
                save_bid(storage, &bid)?;
            }
            a.settlement.cursor = end;
            if end == a.commit_count {
                a.settlement.phase = SettlementPhase::Finalize;
            }
        }
        SettlementPhase::Finalize => {
            let clearing = a
                .settlement
                .clearing
                .as_ref()
                .ok_or_else(|| StdError::generic_err("missing clearing"))?;
            validate_clearing(&a, clearing)?;
            let sold = target(&a)?;
            let unsold_sale = a
                .terms
                .supply_lots
                .checked_sub(sold)?
                .checked_mul(a.terms.lot_size)?;
            require(
                a.settlement.cursor == a.commit_count
                    && a.settlement.residual.is_zero()
                    && a.settlement.base_allocated <= sold
                    && a.settlement.final_allocated == sold
                    && a.settlement.bidder_sale_total == sold.checked_mul(a.terms.lot_size)?
                    && a.settlement.seller_quote == sold.checked_mul(clearing.price)?
                    && a.settlement
                        .bidder_refund_total
                        .checked_add(a.settlement.seller_quote)?
                        == a.total_collateral
                    && a.settlement.bidder_sale_total.checked_add(unsold_sale)?
                        == a.terms.supply_lots.checked_mul(a.terms.lot_size)?,
            )?;
            a.state = BusinessState::Finalized;
            a.settlement.phase = SettlementPhase::Done;
            save_auction(storage, &a)?;
            return Ok(BatchProgress::Finalized);
        }
        SettlementPhase::Done => return Err(ContractError::InvalidPhase),
    }
    save_auction(storage, &a)?;
    Ok(BatchProgress::Continue)
}

fn validate_clearing(a: &AuctionRecord, c: &Clearing) -> Result<(), ContractError> {
    match c.marginal_tick {
        None => require(
            a.total_demand <= Uint256::from(a.terms.supply_lots)
                && c.price == a.terms.reserve_price
                && c.marginal_lots.is_zero()
                && c.marginal_demand.is_zero(),
        ),
        Some(tick) => require(
            a.total_demand > Uint256::from(a.terms.supply_lots)
                && c.price == math::price_at(&a.terms, tick)?
                && !c.marginal_lots.is_zero()
                && a.settlement.higher_demand < Uint256::from(a.terms.supply_lots)
                && c.marginal_lots
                    == a.terms
                        .supply_lots
                        .checked_sub(a.settlement.higher_demand.try_into()?)?
                && c.marginal_demand >= Uint256::from(c.marginal_lots),
        ),
    }
}

fn validate_reveal(a: &AuctionRecord, b: &BidRecord, r: &RevealedBid) -> Result<(), ContractError> {
    require(
        !r.quantity_lots.is_zero()
            && r.quantity_lots <= a.terms.supply_lots
            && r.quantity_lots
                .checked_mul(math::price_at(&a.terms, r.price_tick)?)?
                <= b.collateral,
    )
}
