use crate::msg::AuctionTerms;
use cosmwasm_std::{
    from_json, to_json_vec, Addr, Binary, Decimal, StdError, StdResult, Storage, Uint128, Uint256,
};
use serde::{Deserialize, Serialize};

pub const CONFIG_KEY: &[u8] = b"auction/v1/config";
pub const AUCTION_KEY: &[u8] = b"auction/v1/auction";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusinessState {
    Active,
    Finalized,
    Aborted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementPhase {
    ScanPrice,
    AllocateBase,
    BuildClaims,
    Finalize,
    Done,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clearing {
    pub price: Uint128,
    pub marginal_tick: Option<u16>,
    pub marginal_lots: Uint128,
    pub marginal_demand: Uint256,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BidEntitlement {
    pub sale: Uint128,
    pub quote_refund: Uint128,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SellerEntitlement {
    pub sale: Uint128,
    pub quote: Uint128,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub seller: Addr,
    pub sale_denom: String,
    pub quote_denom: String,
    pub fee_denom: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    pub gas_per_poll: u64,
    pub max_gas_price: Decimal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaimTotals {
    pub bidder_sale: Uint128,
    pub bidder_quote: Uint128,
    pub seller_sale: Uint128,
    pub seller_quote: Uint128,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RevealedBid {
    pub quantity_lots: Uint128,
    pub price_tick: u16,
    pub beneficiary: Addr,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuctionRecord {
    pub auction_id: u64,
    pub task_id: Option<u64>,
    pub created_at: u64,
    pub terms: AuctionTerms,
    pub state: BusinessState,
    pub commit_count: u16,
    pub total_collateral: Uint128,
    pub total_demand: Uint256,
    pub settlement: SettlementProgress,
    pub claimed: ClaimTotals,
    pub seller_claimed: bool,
    pub budget: Budget,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BidRecord {
    pub index: u16,
    pub bidder: Addr,
    pub digest: Binary,
    pub collateral: Uint128,
    pub reveal: Option<RevealedBid>,
    pub allocated_lots: Uint128,
    pub entitlement: Option<BidEntitlement>,
    pub claimed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettlementProgress {
    pub phase: SettlementPhase,
    pub cursor: u16,
    pub higher_demand: Uint256,
    pub clearing: Option<Clearing>,
    pub base_allocated: Uint128,
    pub residual: Uint128,
    pub final_allocated: Uint128,
    pub bidder_sale_total: Uint128,
    pub bidder_refund_total: Uint128,
    pub seller_quote: Uint128,
}

pub fn load_config(storage: &dyn Storage) -> StdResult<Config> {
    from_json(
        storage
            .get(CONFIG_KEY)
            .ok_or_else(|| StdError::not_found("auction config"))?,
    )
}
pub fn load_auction(storage: &dyn Storage) -> StdResult<AuctionRecord> {
    from_json(
        storage
            .get(AUCTION_KEY)
            .ok_or_else(|| StdError::not_found("auction"))?,
    )
}
pub fn save_auction(storage: &mut dyn Storage, auction: &AuctionRecord) -> StdResult<()> {
    storage.set(AUCTION_KEY, &to_json_vec(auction)?);
    Ok(())
}
pub fn load_bid(storage: &dyn Storage, index: u16) -> StdResult<BidRecord> {
    let bid: BidRecord = from_json(
        storage
            .get(&indexed_key(b"auction/v1/bid/", index))
            .ok_or_else(|| StdError::not_found("auction bid"))?,
    )?;
    if bid.index != index {
        return Err(StdError::generic_err("bid index mismatch"));
    }
    Ok(bid)
}
pub fn save_bid(storage: &mut dyn Storage, bid: &BidRecord) -> StdResult<()> {
    let key = indexed_key(b"auction/v1/bid/", bid.index);
    if let Some(data) = storage.get(&key) {
        let previous: BidRecord = from_json(data)?;
        if previous.index != bid.index || previous.bidder != bid.bidder {
            return Err(StdError::generic_err("bid index already bound"));
        }
    }
    if let Some(index) = bid_index(storage, &bid.bidder)? {
        if index != bid.index {
            return Err(StdError::generic_err("bidder already bound"));
        }
    }
    // Finish every fallible validation/serialization before mutating either key.
    let data = to_json_vec(bid)?;
    storage.set(&key, &data);
    storage.set(&bidder_key(&bid.bidder), &bid.index.to_be_bytes());
    Ok(())
}
pub fn bid_index(storage: &dyn Storage, bidder: &Addr) -> StdResult<Option<u16>> {
    let Some(value) = storage.get(&bidder_key(bidder)) else {
        return Ok(None);
    };
    let bytes: [u8; 2] = value
        .try_into()
        .map_err(|_| StdError::generic_err("invalid bidder index"))?;
    let index = u16::from_be_bytes(bytes);
    let bid = load_bid(storage, index)?;
    if bid.bidder != *bidder {
        return Err(StdError::generic_err("bidder index mismatch"));
    }
    Ok(Some(index))
}
pub fn load_demand(storage: &dyn Storage, tick: u16) -> StdResult<Uint256> {
    storage
        .get(&indexed_key(b"auction/v1/demand/", tick))
        .map(from_json)
        .transpose()
        .map(|value| value.unwrap_or_default())
}
pub fn save_demand(storage: &mut dyn Storage, tick: u16, demand: Uint256) -> StdResult<()> {
    storage.set(
        &indexed_key(b"auction/v1/demand/", tick),
        &to_json_vec(&demand)?,
    );
    Ok(())
}
fn indexed_key(prefix: &[u8], index: u16) -> Vec<u8> {
    let mut key = prefix.to_vec();
    key.extend_from_slice(&index.to_be_bytes());
    key
}
fn bidder_key(bidder: &Addr) -> Vec<u8> {
    let mut key = b"auction/v1/bidder/".to_vec();
    key.extend_from_slice(bidder.as_bytes());
    key
}
