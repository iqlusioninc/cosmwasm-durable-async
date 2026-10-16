use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] cosmwasm_std::StdError),
    #[error("{0}")]
    Overflow(#[from] cosmwasm_std::OverflowError),
    #[error("{0}")]
    ConversionOverflow(#[from] cosmwasm_std::ConversionOverflowError),
    #[error("{0}")]
    DivideByZero(#[from] cosmwasm_std::DivideByZeroError),
    #[error("invalid auction terms")]
    InvalidTerms,
    #[error("price tick is outside the auction grid")]
    InvalidTick,
    #[error("invalid commitment encoding")]
    InvalidCommitment,
    #[error("unauthorized")]
    Unauthorized,
    #[error("invalid denomination configuration")]
    InvalidConfig,
    #[error("unexpected attached funds")]
    UnexpectedFunds,
    #[error("attached funds must exactly match the deposit")]
    ExactFunds,
    #[error("auction already created")]
    AlreadyCreated,
    #[error("invalid registration reply")]
    InvalidReply,
    #[error("auction is not active in this phase")]
    InvalidPhase,
    #[error("bid already committed or revealed")]
    DuplicateBid,
    #[error("bid admission limit reached")]
    BidLimit,
    #[error("invalid bid")]
    InvalidBid,
    #[error("task identity does not match auction")]
    InvalidTask,
    #[error("gas budget cannot decrease after a commitment")]
    BudgetReduction,
    #[error("auction cannot be cancelled after a commitment")]
    HasCommitments,
    #[error("principal is not claimable")]
    NotClaimable,
    #[error("claim already paid")]
    AlreadyClaimed,
}
