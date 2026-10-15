use cosmwasm_std::StdError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),
    #[error("unexpected instantiate funds")]
    UnexpectedFunds,
    #[error("invalid native denomination")]
    InvalidDenom,
    #[error("input and output denominations must differ")]
    SameDenom,
    #[error("exchange ratio must be positive")]
    InvalidRatio,
    #[error("swap requires positive input")]
    ZeroInput,
    #[error("swap requires positive minimum output")]
    ZeroMinimum,
    #[error("wrong input denomination")]
    InputDenom,
    #[error("wrong output denomination")]
    OutputDenom,
    #[error("attached funds must be the exact input coin")]
    ExactFunds,
    #[error("zero output after rounding")]
    ZeroOutput,
    #[error("unrepresentable output")]
    QuoteOverflow,
    #[error("output is below minimum output")]
    MinimumOutput,
    #[error("insufficient liquidity")]
    InsufficientLiquidity,
    #[error("duplicate swap")]
    DuplicateSwap,
    #[error("encoded receipt exceeds 2048 bytes")]
    ReceiptTooLarge,
}
