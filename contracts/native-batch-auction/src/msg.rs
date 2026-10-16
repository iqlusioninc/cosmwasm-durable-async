use cosmwasm_std::{Binary, Coin, Decimal, Uint128};
use native_async::{AsyncExecuteMsg, AsyncQueryMsg};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuctionTerms {
    pub supply_lots: Uint128,
    pub lot_size: Uint128,
    pub reserve_price: Uint128,
    pub tick_size: Uint128,
    pub tick_count: u16,
    pub max_bids: u16,
    pub batch_size: u16,
    pub commit_end: u64,
    pub reveal_end: u64,
    pub expires_at: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstantiateMsg {
    pub sale_denom: String,
    pub quote_denom: String,
    pub fee_denom: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateAuctionRequest {
    pub terms: AuctionTerms,
    pub gas: Coin,
    pub gas_per_poll: u64,
    pub max_gas_price: Decimal,
    pub allocated_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RevealBidRequest {
    pub quantity_lots: Uint128,
    pub price_tick: u16,
    pub beneficiary: String,
    pub salt: Binary,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecuteMsg {
    CreateAuction(CreateAuctionRequest),
    CommitBid {
        digest: Binary,
        collateral: Uint128,
    },
    RevealBid(RevealBidRequest),
    ClaimBid {
        bidder: String,
    },
    ClaimSeller {},
    Abort {},
    Async(AsyncExecuteMsg),
    UpdateBudget {
        gas_per_poll: u64,
        max_gas_price: Decimal,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryMsg {
    Auction {},
    Bid {
        bidder: String,
    },
    Bids {
        start_after: Option<u16>,
        limit: Option<u16>,
    },
    Task {},
    Async(AsyncQueryMsg),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateAuctionResponse {
    pub auction_id: u64,
    pub task_id: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkflowInput {
    pub auction_id: u64,
}
