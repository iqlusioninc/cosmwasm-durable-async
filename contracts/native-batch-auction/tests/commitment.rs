use cosmwasm_std::{testing::MockApi, Addr, Api, Uint128};
use native_batch_auction::commitment::{commitment_bytes, commitment_digest, CommitmentInput};
use serde_json::Value;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn fixed_vectors_pin_field_order_utf8_lengths_and_every_integer_width() {
    let vectors: Vec<Value> =
        serde_json::from_str(include_str!("../../../fixtures/auction-commitments.json")).unwrap();
    for v in vectors {
        let api = MockApi::default().with_prefix("cosmos");
        for field in ["contract", "bidder", "beneficiary"] {
            assert_eq!(
                api.addr_validate(v[field].as_str().unwrap())
                    .unwrap()
                    .as_str(),
                v[field].as_str().unwrap()
            );
        }
        let contract = Addr::unchecked(v["contract"].as_str().unwrap());
        let bidder = Addr::unchecked(v["bidder"].as_str().unwrap());
        let beneficiary = Addr::unchecked(v["beneficiary"].as_str().unwrap());
        let salt = unhex(v["salt_hex"].as_str().unwrap());
        let input = CommitmentInput {
            chain_id: v["chain_id"].as_str().unwrap(),
            contract: &contract,
            bidder: &bidder,
            beneficiary: &beneficiary,
            auction_id: v["auction_id"].as_str().unwrap().parse().unwrap(),
            collateral: Uint128::new(v["collateral"].as_str().unwrap().parse().unwrap()),
            quantity_lots: Uint128::new(v["quantity_lots"].as_str().unwrap().parse().unwrap()),
            price_tick: v["price_tick"].as_u64().unwrap() as u16,
            salt: &salt,
        };
        assert_eq!(
            hex(&commitment_bytes(&input).unwrap()),
            v["bytes_hex"].as_str().unwrap()
        );
        assert_eq!(
            hex(&commitment_digest(&input).unwrap()),
            v["digest_hex"].as_str().unwrap()
        );
    }
}

#[test]
fn every_domain_field_and_bid_term_binds_the_digest() {
    let c = Addr::unchecked("contract");
    let b = Addr::unchecked("bidder");
    let recipient = Addr::unchecked("recipient");
    let other = Addr::unchecked("other");
    let salt = [0u8; 32];
    let other_salt = [1u8; 32];
    let input = CommitmentInput {
        chain_id: "chain-1",
        contract: &c,
        bidder: &b,
        beneficiary: &recipient,
        auction_id: 1,
        collateral: Uint128::new(8),
        quantity_lots: Uint128::new(2),
        price_tick: 1,
        salt: &salt,
    };
    let original = commitment_digest(&input).unwrap();
    for field in 0..9 {
        let mut changed = CommitmentInput { ..input };
        match field {
            0 => changed.chain_id = "chain-2",
            1 => changed.contract = &other,
            2 => changed.bidder = &other,
            3 => changed.beneficiary = &other,
            4 => changed.auction_id = 2,
            5 => changed.collateral = Uint128::new(9),
            6 => changed.quantity_lots = Uint128::new(3),
            7 => changed.price_tick = 2,
            _ => changed.salt = &other_salt,
        };
        assert_ne!(
            commitment_digest(&changed).unwrap(),
            original,
            "field {field}"
        );
    }
}

#[test]
fn salt_requires_exactly_32_bytes() {
    let addr = Addr::unchecked("canonical");
    for len in [0, 31, 32, 33] {
        let salt = vec![0; len];
        let i = CommitmentInput {
            chain_id: "chain",
            contract: &addr,
            bidder: &addr,
            beneficiary: &addr,
            auction_id: 1,
            collateral: Uint128::one(),
            quantity_lots: Uint128::one(),
            price_tick: 0,
            salt: &salt,
        };
        assert_eq!(commitment_bytes(&i).is_ok(), len == 32);
    }
}
