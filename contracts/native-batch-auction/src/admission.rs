use crate::{contract::*, error::ContractError, msg::*, state::*};
use cosmwasm_std::*;
use native_async::{RuntimeMsg, RuntimeQuery};
use std::collections::BTreeMap;

pub fn create(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    info: MessageInfo,
    r: CreateAuctionRequest,
) -> Result<Response<RuntimeMsg>, ContractError> {
    let config = load_config(deps.storage)?;
    seller_only(&info, &config)?;
    if deps.storage.get(AUCTION_KEY).is_some() {
        return Err(ContractError::AlreadyCreated);
    }
    crate::math::validate_terms(&r.terms, env.block.height)?;
    if r.gas.amount.is_zero()
        || r.gas.denom != config.fee_denom
        || r.gas_per_poll == 0
        || r.max_gas_price.is_zero()
        || r.allocated_bytes < crate::MIN_ALLOCATED_BYTES
    {
        return Err(ContractError::InvalidTerms);
    }
    let principal = r.terms.supply_lots.checked_mul(r.terms.lot_size)?;
    let mut expected = BTreeMap::from([(config.sale_denom, principal)]);
    let gas = expected.entry(r.gas.denom.clone()).or_default();
    *gas = gas.checked_add(r.gas.amount)?;
    exact_funds(&info, &expected)?;
    let auction = AuctionRecord {
        auction_id: 1,
        task_id: None,
        created_at: env.block.height,
        terms: r.terms.clone(),
        state: BusinessState::Active,
        commit_count: 0,
        total_collateral: Uint128::zero(),
        total_demand: Uint256::zero(),
        settlement: SettlementProgress {
            phase: SettlementPhase::ScanPrice,
            cursor: r.terms.tick_count,
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
            gas_per_poll: r.gas_per_poll,
            max_gas_price: r.max_gas_price,
        },
    };
    save_auction(deps.storage, &auction)?;
    Ok(Response::new().add_submessage(
        SubMsg::reply_on_success(
            CosmosMsg::Custom(RuntimeMsg::Register {
                control: config.seller.to_string(),
                refund: config.seller.to_string(),
                gas_per_poll: r.gas_per_poll,
                max_gas_price: r.max_gas_price,
                expires_at: r.terms.expires_at,
                funds: r.gas,
                program: program(1)?,
                allocated_bytes: r.allocated_bytes,
            }),
            REGISTER_REPLY,
        )
        .with_payload(to_json_binary(&RegisterReplyPayload { auction_id: 1 })?),
    ))
}
pub fn commit(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    info: MessageInfo,
    digest: Binary,
    collateral: Uint128,
) -> Result<Response<RuntimeMsg>, ContractError> {
    let config = load_config(deps.storage)?;
    let mut auction = load_auction(deps.storage)?;
    active_before_expiry(&auction, env.block.height)?;
    if env.block.height >= auction.terms.commit_end {
        return Err(ContractError::InvalidPhase);
    }
    let bidder = canonical_address(deps.api, info.sender.as_str())?;
    if bid_index(deps.storage, &bidder)?.is_some() {
        return Err(ContractError::DuplicateBid);
    }
    if auction.commit_count >= auction.terms.max_bids {
        return Err(ContractError::BidLimit);
    }
    if digest.len() != 32 || collateral.is_zero() {
        return Err(ContractError::InvalidBid);
    }
    exact_funds(&info, &BTreeMap::from([(config.quote_denom, collateral)]))?;
    let total = auction.total_collateral.checked_add(collateral)?;
    let count = auction
        .commit_count
        .checked_add(1)
        .ok_or(ContractError::BidLimit)?;
    let bid = BidRecord {
        index: auction.commit_count,
        bidder,
        digest,
        collateral,
        reveal: None,
        allocated_lots: Uint128::zero(),
        entitlement: None,
        claimed: false,
    };
    auction.total_collateral = total;
    auction.commit_count = count;
    save_bid(deps.storage, &bid)?;
    save_auction(deps.storage, &auction)?;
    Ok(Response::new())
}
pub fn reveal(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    info: MessageInfo,
    r: RevealBidRequest,
) -> Result<Response<RuntimeMsg>, ContractError> {
    let mut auction = load_auction(deps.storage)?;
    active_before_expiry(&auction, env.block.height)?;
    if env.block.height < auction.terms.commit_end || env.block.height >= auction.terms.reveal_end {
        return Err(ContractError::InvalidPhase);
    }
    let bidder = canonical_address(deps.api, info.sender.as_str())?;
    let index = bid_index(deps.storage, &bidder)?.ok_or(ContractError::InvalidBid)?;
    let mut bid = load_bid(deps.storage, index)?;
    if bid.reveal.is_some() {
        return Err(ContractError::DuplicateBid);
    }
    if r.quantity_lots.is_zero()
        || r.quantity_lots > auction.terms.supply_lots
        || r.salt.len() != 32
    {
        return Err(ContractError::InvalidBid);
    }
    let price = crate::math::price_at(&auction.terms, r.price_tick)?;
    if r.quantity_lots.checked_mul(price)? > bid.collateral {
        return Err(ContractError::InvalidBid);
    }
    let beneficiary = canonical_address(deps.api, &r.beneficiary)?;
    let digest = crate::commitment::commitment_digest(&crate::commitment::CommitmentInput {
        chain_id: &env.block.chain_id,
        contract: &env.contract.address,
        bidder: &bidder,
        beneficiary: &beneficiary,
        auction_id: auction.auction_id,
        collateral: bid.collateral,
        quantity_lots: r.quantity_lots,
        price_tick: r.price_tick,
        salt: r.salt.as_slice(),
    })?;
    if bid.digest.as_slice() != digest {
        return Err(ContractError::InvalidCommitment);
    }
    // Validate digest and every arithmetic result before the first state write.
    let demand =
        load_demand(deps.storage, r.price_tick)?.checked_add(Uint256::from(r.quantity_lots))?;
    auction.total_demand = auction
        .total_demand
        .checked_add(Uint256::from(r.quantity_lots))?;
    bid.reveal = Some(RevealedBid {
        quantity_lots: r.quantity_lots,
        price_tick: r.price_tick,
        beneficiary,
    });
    save_bid(deps.storage, &bid)?;
    save_demand(deps.storage, r.price_tick, demand)?;
    save_auction(deps.storage, &auction)?;
    Ok(Response::new())
}
fn exact_funds(
    info: &MessageInfo,
    expected: &BTreeMap<String, Uint128>,
) -> Result<(), ContractError> {
    let mut attached = BTreeMap::new();
    for c in &info.funds {
        if c.amount.is_zero() || attached.insert(c.denom.clone(), c.amount).is_some() {
            return Err(ContractError::ExactFunds);
        }
    }
    if &attached != expected {
        return Err(ContractError::ExactFunds);
    }
    Ok(())
}
