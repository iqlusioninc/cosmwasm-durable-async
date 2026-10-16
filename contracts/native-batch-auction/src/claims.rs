use crate::{contract::canonical_address, error::ContractError, state::*};
use cosmwasm_std::*;
use native_async::{RuntimeMsg, RuntimeQuery};
pub fn bid(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    bidder: String,
) -> Result<Response<RuntimeMsg>, ContractError> {
    let config = load_config(deps.storage)?;
    let mut auction = load_auction(deps.storage)?;
    make_claimable(&mut auction, env.block.height)?;
    let bidder = canonical_address(deps.api, &bidder)?;
    let index = bid_index(deps.storage, &bidder)?.ok_or(ContractError::InvalidBid)?;
    let mut bid = load_bid(deps.storage, index)?;
    if bid.claimed {
        return Err(ContractError::AlreadyClaimed);
    }
    let entitlement = if auction.state == BusinessState::Aborted {
        // Never read provisional allocation/entitlement on the abort path.
        BidEntitlement {
            sale: Uint128::zero(),
            quote_refund: bid.collateral,
        }
    } else {
        bid.entitlement.clone().ok_or(ContractError::NotClaimable)?
    };
    let sale_recipient = if entitlement.sale.is_zero() {
        bidder.clone()
    } else {
        bid.reveal
            .as_ref()
            .ok_or(ContractError::InvalidBid)?
            .beneficiary
            .clone()
    };
    auction.claimed.bidder_sale = auction.claimed.bidder_sale.checked_add(entitlement.sale)?;
    auction.claimed.bidder_quote = auction
        .claimed
        .bidder_quote
        .checked_add(entitlement.quote_refund)?;
    bid.claimed = true;
    save_bid(deps.storage, &bid)?;
    save_auction(deps.storage, &auction)?;
    let response = send(
        Response::new(),
        &sale_recipient,
        config.sale_denom,
        entitlement.sale,
    );
    Ok(send(
        response,
        &bidder,
        config.quote_denom,
        entitlement.quote_refund,
    ))
}
pub fn seller(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
) -> Result<Response<RuntimeMsg>, ContractError> {
    let config = load_config(deps.storage)?;
    let mut auction = load_auction(deps.storage)?;
    make_claimable(&mut auction, env.block.height)?;
    if auction.seller_claimed {
        return Err(ContractError::AlreadyClaimed);
    }
    let entitlement = if auction.state == BusinessState::Aborted {
        SellerEntitlement {
            sale: auction
                .terms
                .supply_lots
                .checked_mul(auction.terms.lot_size)?,
            quote: Uint128::zero(),
        }
    } else {
        SellerEntitlement {
            sale: auction
                .terms
                .supply_lots
                .checked_sub(auction.settlement.final_allocated)?
                .checked_mul(auction.terms.lot_size)?,
            quote: auction.settlement.seller_quote,
        }
    };
    auction.claimed.seller_sale = auction.claimed.seller_sale.checked_add(entitlement.sale)?;
    auction.claimed.seller_quote = auction
        .claimed
        .seller_quote
        .checked_add(entitlement.quote)?;
    auction.seller_claimed = true;
    save_auction(deps.storage, &auction)?;
    let response = send(
        Response::new(),
        &config.seller,
        config.sale_denom,
        entitlement.sale,
    );
    Ok(send(
        response,
        &config.seller,
        config.quote_denom,
        entitlement.quote,
    ))
}
pub fn abort(deps: DepsMut<RuntimeQuery>, env: Env) -> Result<Response<RuntimeMsg>, ContractError> {
    let mut auction = load_auction(deps.storage)?;
    if auction.state != BusinessState::Active || env.block.height < auction.terms.expires_at {
        return Err(ContractError::NotClaimable);
    }
    auction.state = BusinessState::Aborted;
    save_auction(deps.storage, &auction)?;
    Ok(Response::new())
}
fn make_claimable(auction: &mut AuctionRecord, height: u64) -> Result<(), ContractError> {
    if auction.state == BusinessState::Active {
        if height < auction.terms.expires_at {
            return Err(ContractError::NotClaimable);
        }
        auction.state = BusinessState::Aborted;
    }
    Ok(())
}
fn send(
    response: Response<RuntimeMsg>,
    recipient: &Addr,
    denom: String,
    amount: Uint128,
) -> Response<RuntimeMsg> {
    if amount.is_zero() {
        response
    } else {
        response.add_message(BankMsg::Send {
            to_address: recipient.to_string(),
            amount: vec![Coin { denom, amount }],
        })
    }
}
