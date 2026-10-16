mod support;
use cosmwasm_std::*;
use cw_multi_test::Executor;
use native_batch_auction::{msg::*, state::*};
use support::AuctionSuite;

fn finalized_fixture() -> (AuctionSuite, Addr) {
    let mut s = AuctionSuite::new();
    s.create().unwrap();
    let b = s.bidder("claim-bidder");
    s.commit(&b, 2, 0, 20).unwrap();
    s.height(10);
    s.reveal(&b, 2, 0).unwrap();
    s.settle();
    (s, b)
}

#[test]
fn provisional_claims_stay_locked_even_if_host_has_failed() {
    // Break: treating host failure or provisional allocation as principal authority.
    let mut s = AuctionSuite::new();
    s.create().unwrap();
    let b = s.bidder("locked");
    s.commit(&b, 2, 0, 20).unwrap();
    s.block(2).unwrap();
    assert!(s
        .execute(
            s.sponsor.clone(),
            &ExecuteMsg::ClaimBid {
                bidder: b.to_string()
            },
            &[]
        )
        .is_err());
    assert!(s
        .execute(s.seller.clone(), &ExecuteMsg::ClaimSeller {}, &[])
        .is_err());
    assert!(s
        .execute(s.sponsor.clone(), &ExecuteMsg::Abort {}, &[])
        .is_err());
    assert!(!s.bid(&b).unwrap().claimed);
}

#[test]
fn finalized_fixture_claims_are_permissionless_immutable_and_conserve_principal() {
    // Break: paying caller, refunding beneficiary, double claim, or donation-based payout.
    let (mut s, b) = finalized_fixture();
    let bidder_before = s.balance(&b, "uquote");
    let seller_quote = s.balance(&s.seller, "uquote");
    let seller_sale = s.balance(&s.seller, "usale");
    s.app
        .send_tokens(
            s.sponsor.clone(),
            s.contract.clone(),
            &[coin(7, "usale"), coin(9, "uquote")],
        )
        .unwrap();
    s.execute(
        s.sponsor.clone(),
        &ExecuteMsg::ClaimBid {
            bidder: b.to_string(),
        },
        &[],
    )
    .unwrap();
    assert_eq!(s.balance(&s.beneficiary, "usale"), Uint128::new(20));
    assert_eq!(s.balance(&b, "uquote"), bidder_before + Uint128::new(16));
    assert!(s
        .execute(
            s.sponsor.clone(),
            &ExecuteMsg::ClaimBid {
                bidder: b.to_string()
            },
            &[]
        )
        .is_err());
    s.execute(b.clone(), &ExecuteMsg::ClaimSeller {}, &[])
        .unwrap();
    assert_eq!(
        s.balance(&s.seller, "uquote"),
        seller_quote + Uint128::new(4)
    );
    assert_eq!(
        s.balance(&s.seller, "usale"),
        seller_sale + Uint128::new(30)
    );
    assert!(s.execute(b, &ExecuteMsg::ClaimSeller {}, &[]).is_err());
    assert_eq!(s.balance(&s.contract, "usale"), Uint128::new(7));
    assert_eq!(s.balance(&s.contract, "uquote"), Uint128::new(9));
    let a = s.auction().auction;
    assert_eq!(
        a.claimed.bidder_sale + a.claimed.seller_sale,
        Uint128::new(50)
    );
    assert_eq!(
        a.claimed.bidder_quote + a.claimed.seller_quote,
        Uint128::new(20)
    );
}

#[test]
fn failed_second_bid_transfer_rolls_back_first_transfer_flag_and_totals() {
    // Break: split transactions or claim accounting surviving second payout failure.
    let (mut s, b) = finalized_fixture();
    let before = [
        s.balance(&s.contract, "usale"),
        s.balance(&s.contract, "uquote"),
        s.balance(&s.beneficiary, "usale"),
        s.balance(&b, "uquote"),
    ];
    let auction = s.auction().auction;
    s.reject_bank(&b, "uquote");
    let error = s
        .execute(
            s.sponsor.clone(),
            &ExecuteMsg::ClaimBid {
                bidder: b.to_string(),
            },
            &[],
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("injected bank rejection"));
    assert_eq!(
        [
            s.balance(&s.contract, "usale"),
            s.balance(&s.contract, "uquote"),
            s.balance(&s.beneficiary, "usale"),
            s.balance(&b, "uquote")
        ],
        before
    );
    assert!(!s.bid(&b).unwrap().claimed);
    assert_eq!(s.auction().auction, auction);
    s.allow_bank();
    s.execute(
        s.sponsor.clone(),
        &ExecuteMsg::ClaimBid {
            bidder: b.to_string(),
        },
        &[],
    )
    .unwrap();
}

#[test]
fn failed_second_seller_transfer_rolls_back_inventory_and_claim_flag() {
    // Break: non-atomic seller inventory/proceeds payout.
    let (mut s, _) = finalized_fixture();
    let before = [
        s.balance(&s.seller, "usale"),
        s.balance(&s.seller, "uquote"),
        s.balance(&s.contract, "usale"),
        s.balance(&s.contract, "uquote"),
    ];
    s.reject_bank(&s.seller.clone(), "uquote");
    let error = s
        .execute(s.sponsor.clone(), &ExecuteMsg::ClaimSeller {}, &[])
        .unwrap_err();
    assert!(format!("{error:#}").contains("injected bank rejection"));
    assert_eq!(
        [
            s.balance(&s.seller, "usale"),
            s.balance(&s.seller, "uquote"),
            s.balance(&s.contract, "usale"),
            s.balance(&s.contract, "uquote")
        ],
        before
    );
    assert!(!s.auction().auction.seller_claimed);
    s.allow_bank();
    s.execute(s.sponsor.clone(), &ExecuteMsg::ClaimSeller {}, &[])
        .unwrap();
}

#[test]
fn expiry_lazy_abort_ignores_every_provisional_entitlement_and_refunds_unrevealed() {
    // Break: requiring host liveness or honoring provisional allocations after abort.
    for fee in ["ugas", "uquote", "usale"] {
        let mut s = AuctionSuite::with_fee_denom(fee);
        s.create().unwrap();
        let b = s.bidder("abort-bidder");
        s.commit(&b, 2, 0, 20).unwrap();
        {
            let mut storage = s.app.contract_storage_mut(&s.contract);
            let mut bid = load_bid(&*storage, 0).unwrap();
            bid.allocated_lots = Uint128::new(2);
            bid.entitlement = Some(BidEntitlement {
                sale: Uint128::new(20),
                quote_refund: Uint128::new(16),
            });
            save_bid(&mut *storage, &bid).unwrap();
        }
        s.block(71).unwrap();
        s.reconstruct().unwrap();
        let before = s.balance(&b, "uquote");
        s.execute(
            s.sponsor.clone(),
            &ExecuteMsg::ClaimBid {
                bidder: b.to_string(),
            },
            &[],
        )
        .unwrap();
        assert_eq!(s.auction().auction.state, BusinessState::Aborted);
        assert_eq!(s.balance(&b, "uquote"), before + Uint128::new(20));
        assert_eq!(s.balance(&s.beneficiary, "usale"), Uint128::zero());
        let before = s.balance(&s.seller, "usale");
        s.execute(b, &ExecuteMsg::ClaimSeller {}, &[]).unwrap();
        assert_eq!(s.balance(&s.seller, "usale"), before + Uint128::new(50));
    }
}

#[test]
fn finalized_fixture_claims_survive_expiry_pruning_and_reconstruction() {
    // Break: expiry overwriting Finalized or claims depending on host task retention.
    let (mut s, b) = finalized_fixture();
    s.block(71).unwrap();
    s.reconstruct().unwrap();
    assert!(s
        .execute(s.sponsor.clone(), &ExecuteMsg::Abort {}, &[])
        .is_err());
    s.execute(
        s.sponsor.clone(),
        &ExecuteMsg::ClaimBid {
            bidder: b.to_string(),
        },
        &[],
    )
    .unwrap();
    assert_eq!(s.auction().auction.state, BusinessState::Finalized);
    assert_eq!(s.balance(&s.beneficiary, "usale"), Uint128::new(20));
}

#[test]
fn two_instances_cannot_use_each_others_liabilities_or_host_ids() {
    // Break: treating native task IDs or custody as globally shared by applications.
    let mut s = AuctionSuite::new();
    s.create().unwrap();
    let first = s.contract.clone();
    let code = s.app.store_code(support::auction_contract());
    let second = s
        .app
        .instantiate_contract(
            code,
            s.sponsor.clone(),
            &InstantiateMsg {
                sale_denom: "usale".into(),
                quote_denom: "uquote".into(),
                fee_denom: "ugas".into(),
            },
            &[],
            "second",
            None,
        )
        .unwrap();
    s.app.init_modules(|r, _, _| {
        r.custom.identities.insert(
            second.to_string(),
            Binary::from(b"immutable-rust-native-auction-v1".as_slice()),
        );
    });
    let r = s.request();
    let f = s.funds(&r);
    s.app
        .execute_contract(
            s.sponsor.clone(),
            second.clone(),
            &ExecuteMsg::CreateAuction(r),
            &f,
        )
        .unwrap();
    let b = s.bidder("isolation");
    s.commit(&b, 2, 0, 20).unwrap();
    s.height(60);
    s.execute(
        s.sponsor.clone(),
        &ExecuteMsg::ClaimBid {
            bidder: b.to_string(),
        },
        &[],
    )
    .unwrap();
    assert_eq!(s.balance(&second, "usale"), Uint128::new(50));
    assert_eq!(s.balance(&second, "uquote"), Uint128::zero());
    assert!(s
        .app
        .execute_contract(
            s.seller.clone(),
            second.clone(),
            &ExecuteMsg::ClaimBid {
                bidder: b.to_string()
            },
            &[]
        )
        .is_err());
    assert_eq!(s.balance(&first, "usale"), Uint128::new(50));
}

#[test]
fn finalized_unrevealed_bid_refunds_full_collateral_without_a_beneficiary() {
    // Break: requiring a reveal for a zero-sale refund or inventing a non-reveal penalty.
    let mut s = AuctionSuite::new();
    s.create().unwrap();
    let b = s.bidder("unrevealed-finalized");
    s.commit(&b, 2, 0, 20).unwrap();
    s.settle();
    let before = s.balance(&b, "uquote");
    s.execute(
        s.sponsor.clone(),
        &ExecuteMsg::ClaimBid {
            bidder: b.to_string(),
        },
        &[],
    )
    .unwrap();
    assert_eq!(s.balance(&b, "uquote"), before + Uint128::new(20));
    assert_eq!(s.balance(&s.beneficiary, "usale"), Uint128::zero());
    s.execute(s.sponsor.clone(), &ExecuteMsg::ClaimSeller {}, &[])
        .unwrap();
    assert_eq!(s.balance(&s.contract, "usale"), Uint128::zero());
    assert_eq!(s.balance(&s.contract, "uquote"), Uint128::zero());
}

#[test]
fn failed_lazy_abort_refund_reverts_abort_state_and_can_be_retried_at_expiry() {
    // Break: lazy abort writes escaping a bank failure, or an E versus E-1 fencepost.
    let mut s = AuctionSuite::new();
    s.create().unwrap();
    let b = s.bidder("lazy-retry");
    s.commit(&b, 2, 0, 20).unwrap();
    s.height(59);
    assert!(s
        .execute(
            s.sponsor.clone(),
            &ExecuteMsg::ClaimBid {
                bidder: b.to_string()
            },
            &[]
        )
        .is_err());
    s.height(60);
    s.reject_bank(&b, "uquote");
    let before = s.auction().auction;
    let error = s
        .execute(
            s.sponsor.clone(),
            &ExecuteMsg::ClaimBid {
                bidder: b.to_string(),
            },
            &[],
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("injected bank rejection"));
    assert_eq!(s.auction().auction, before);
    assert!(!s.bid(&b).unwrap().claimed);
    s.allow_bank();
    s.execute(
        s.sponsor.clone(),
        &ExecuteMsg::ClaimBid {
            bidder: b.to_string(),
        },
        &[],
    )
    .unwrap();
    assert_eq!(s.auction().auction.state, BusinessState::Aborted);
    assert_eq!(s.auction().auction.claimed.bidder_quote, Uint128::new(20));
}
