use cosmwasm_std::StdError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),
    #[error("{0}")]
    Runtime(#[from] durable_runtime::RuntimeError),
    #[error("only the order owner may manage principal or budget")]
    Unauthorized,
    #[error("this call does not accept funds")]
    UnexpectedFunds,
    #[error("invalid DCA configuration")]
    InvalidConfig,
    #[error("invalid order terms")]
    InvalidOrder,
    #[error("attached funds must exactly match principal plus initial gas")]
    ExactFunds,
    #[error("allocation cannot hold the bounded continuation and host reserves")]
    Allocation,
    #[error("invalid registration reply or task binding")]
    InvalidReply,
    #[error("task is not terminal")]
    NotTerminal,
    #[error("order has no remaining principal")]
    EmptyPrincipal,
    #[error("invalid slice settlement or staged task identity")]
    Settlement,
}
