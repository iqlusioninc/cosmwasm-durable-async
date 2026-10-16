use cosmwasm_std::{Uint128, Uint256};
use native_batch_auction::{msg::AuctionTerms, state::Clearing};

pub fn terms() -> AuctionTerms {
    AuctionTerms {
        supply_lots: Uint128::new(5),
        lot_size: Uint128::new(10),
        reserve_price: Uint128::new(2),
        tick_size: Uint128::one(),
        tick_count: 3,
        max_bids: 5,
        batch_size: 2,
        commit_end: 20,
        reveal_end: 30,
        expires_at: 100,
    }
}

/// Independent one-shot oracle: sort the complete book, locate the S-th lot,
/// then apportion at that price. Never scans the production tick buckets.
pub fn oracle(terms: &AuctionTerms, book: &[Option<(u128, u16)>]) -> (Clearing, Vec<u128>) {
    let supply = terms.supply_lots.u128();
    let mut ranked: Vec<_> = book
        .iter()
        .enumerate()
        .filter_map(|(i, bid)| bid.map(|(q, t)| (i, q, t)))
        .collect();
    ranked.sort_by_key(|&(i, _, tick)| (std::cmp::Reverse(tick), i));
    let total: Uint256 = ranked.iter().map(|&(_, q, _)| Uint256::from(q)).sum();
    let mut allocations = vec![0; book.len()];
    if total <= Uint256::from(supply) {
        for &(i, q, _) in &ranked {
            allocations[i] = q;
        }
        return (
            Clearing {
                price: terms.reserve_price,
                marginal_tick: None,
                marginal_lots: Uint128::zero(),
                marginal_demand: Uint256::zero(),
            },
            allocations,
        );
    }
    let mut cumulative = Uint256::zero();
    let marginal_tick = ranked
        .iter()
        .find_map(|&(_, q, t)| {
            cumulative += Uint256::from(q);
            (cumulative >= Uint256::from(supply)).then_some(t)
        })
        .unwrap();
    let higher: Uint256 = ranked
        .iter()
        .filter(|&&(_, _, t)| t > marginal_tick)
        .map(|&(_, q, _)| Uint256::from(q))
        .sum();
    let marginal: Uint256 = ranked
        .iter()
        .filter(|&&(_, _, t)| t == marginal_tick)
        .map(|&(_, q, _)| Uint256::from(q))
        .sum();
    let remaining = Uint256::from(supply) - higher;
    for &(i, q, t) in &ranked {
        allocations[i] = match t.cmp(&marginal_tick) {
            std::cmp::Ordering::Greater => q,
            std::cmp::Ordering::Equal => Uint128::try_from(Uint256::from(q) * remaining / marginal)
                .unwrap()
                .u128(),
            std::cmp::Ordering::Less => 0,
        };
    }
    let mut dust = supply - allocations.iter().sum::<u128>();
    let marginal_count = ranked
        .iter()
        .filter(|&&(_, _, t)| t == marginal_tick)
        .count();
    assert!(dust < marginal_count as u128);
    for (i, bid) in book.iter().enumerate() {
        if let Some((q, t)) = bid {
            if dust > 0 && *t == marginal_tick && allocations[i] < *q {
                allocations[i] += 1;
                dust -= 1;
            }
        }
    }
    assert_eq!(dust, 0);
    (
        Clearing {
            price: Uint128::new(
                terms.reserve_price.u128() + u128::from(marginal_tick) * terms.tick_size.u128(),
            ),
            marginal_tick: Some(marginal_tick),
            marginal_lots: remaining.try_into().unwrap(),
            marginal_demand: marginal,
        },
        allocations,
    )
}
