use crate::state::{AuctionRecord, Config};
use native_async::TaskResponse;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BiddingPhase {
    Commit,
    Reveal,
    Settlement,
    AwaitingAbort,
    Finalized,
    Aborted,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostTaskView {
    Present(Box<TaskResponse>),
    Unavailable { error: String, lease_expired: bool },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuctionView {
    pub config: Config,
    pub auction: AuctionRecord,
    pub phase: BiddingPhase,
    pub host: HostTaskView,
}

use crate::{
    contract::{bound_task, canonical_address},
    msg::QueryMsg,
    state::*,
};
use cosmwasm_std::*;
use native_async::{AsyncQueryMsg, RuntimeQuery, SchedulerQuery};
pub(crate) fn host_task(
    deps: Deps<RuntimeQuery>,
    env: &Env,
    task_id: u64,
) -> StdResult<TaskResponse> {
    deps.querier
        .query(&QueryRequest::Custom(RuntimeQuery::Task {
            contract: env.contract.address.to_string(),
            task_id,
        }))
}
pub fn query(deps: Deps<RuntimeQuery>, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Auction {} => {
            let auction = load_auction(deps.storage)?;
            let host = match bound_task(&auction).and_then(|id| host_task(deps, &env, id)) {
                Ok(task) => HostTaskView::Present(Box::new(task)),
                Err(e) => HostTaskView::Unavailable {
                    error: e.to_string(),
                    lease_expired: env.block.height >= auction.terms.expires_at,
                },
            };
            let phase = match auction.state {
                BusinessState::Finalized => BiddingPhase::Finalized,
                BusinessState::Aborted => BiddingPhase::Aborted,
                BusinessState::Active if env.block.height >= auction.terms.expires_at => {
                    BiddingPhase::AwaitingAbort
                }
                BusinessState::Active if env.block.height >= auction.terms.reveal_end => {
                    BiddingPhase::Settlement
                }
                BusinessState::Active if env.block.height >= auction.terms.commit_end => {
                    BiddingPhase::Reveal
                }
                BusinessState::Active => BiddingPhase::Commit,
            };
            to_json_binary(&AuctionView {
                config: load_config(deps.storage)?,
                auction,
                phase,
                host,
            })
        }
        QueryMsg::Bid { bidder } => {
            let bidder = canonical_address(deps.api, &bidder)?;
            to_json_binary(
                &bid_index(deps.storage, &bidder)?
                    .map(|i| load_bid(deps.storage, i))
                    .transpose()?,
            )
        }
        QueryMsg::Bids { start_after, limit } => {
            let limit = limit.unwrap_or(16);
            if !(1..=32).contains(&limit) {
                return Err(StdError::generic_err("bid page limit must be 1..=32"));
            }
            let auction = load_auction(deps.storage)?;
            let start = start_after.map(|i| u32::from(i) + 1).unwrap_or(0);
            let end = (start + u32::from(limit)).min(u32::from(auction.commit_count));
            let bids = (start..end)
                .map(|i| load_bid(deps.storage, i as u16))
                .collect::<StdResult<Vec<_>>>()?;
            to_json_binary(&bids)
        }
        QueryMsg::Task {} => to_json_binary(&host_task(
            deps,
            &env,
            bound_task(&load_auction(deps.storage)?)?,
        )?),
        QueryMsg::Async(AsyncQueryMsg::FundingConfig {}) => {
            to_json_binary(&deps.querier.query::<native_async::FundingConfig>(
                &QueryRequest::Custom(RuntimeQuery::Funding(SchedulerQuery::FundingConfig {})),
            )?)
        }
        QueryMsg::Async(AsyncQueryMsg::TaskFunding { task_id }) => {
            if task_id != bound_task(&load_auction(deps.storage)?)? {
                return Err(StdError::generic_err(
                    "task identity does not match auction",
                ));
            }
            to_json_binary(&deps.querier.query::<native_async::TaskFunding>(
                &QueryRequest::Custom(RuntimeQuery::Funding(SchedulerQuery::TaskFunding {
                    contract: env.contract.address.to_string(),
                    task_id,
                })),
            )?)
        }
    }
}
