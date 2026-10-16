use crate::error::ContractError;
use cosmwasm_std::{Addr, Uint128};
use sha2::{Digest, Sha256};

const DOMAIN: &[u8] = b"cosmwasm/native-batch-auction/bid/v1";

pub struct CommitmentInput<'a> {
    pub chain_id: &'a str,
    pub contract: &'a Addr,
    pub bidder: &'a Addr,
    pub beneficiary: &'a Addr,
    pub auction_id: u64,
    pub collateral: Uint128,
    pub quantity_lots: Uint128,
    pub price_tick: u16,
    pub salt: &'a [u8],
}
pub fn commitment_bytes(input: &CommitmentInput<'_>) -> Result<Vec<u8>, ContractError> {
    if input.salt.len() != 32 {
        return Err(ContractError::InvalidCommitment);
    }
    let mut bytes = DOMAIN.to_vec();
    for value in [
        input.chain_id,
        input.contract.as_str(),
        input.bidder.as_str(),
        input.beneficiary.as_str(),
    ] {
        let length = u32::try_from(value.len()).map_err(|_| ContractError::InvalidCommitment)?;
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(value.as_bytes());
    }
    bytes.extend_from_slice(&input.auction_id.to_be_bytes());
    bytes.extend_from_slice(&input.collateral.u128().to_be_bytes());
    bytes.extend_from_slice(&input.quantity_lots.u128().to_be_bytes());
    bytes.extend_from_slice(&input.price_tick.to_be_bytes());
    bytes.extend_from_slice(input.salt);
    Ok(bytes)
}
pub fn commitment_digest(input: &CommitmentInput<'_>) -> Result<[u8; 32], ContractError> {
    Ok(Sha256::digest(commitment_bytes(input)?).into())
}
