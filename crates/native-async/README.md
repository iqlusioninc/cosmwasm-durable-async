# Native async funding: first implementation increment

This crate implements the proposed contract funding protocol and a reference host escrow keeper. It does **not** add async execution to an existing chain. Contracts using these custom messages require a chain adapter; unmodified wasmd does not understand them.

Implemented:

- Standard `AsyncExecuteMsg::{FundTask, ResumeTask, CancelTask}` and `AsyncQueryMsg::{FundingConfig, TaskFunding}`.
- Contract helpers that forward attached native coins and the actual `MessageInfo.sender`.
- Host-owned, contract-scoped task funding records and monotonic IDs.
- Registration, positive deposits, checked balance arithmetic, explicit resume, cancellation, and refunds.
- Logical expiry checks, ceiling-rounded attempt prices, saved waiting/runnable state, and refund/control isolation from sponsors.
- Transactional tests exercising bank transfers, cross-contract isolation, insufficient balances, overflow, and downstream rollback.

The optional `host` feature is a **reference implementation**, using CosmWasm's storage interface for a portable test harness. A production Cosmos SDK keeper will need equivalent Go state transitions and bank integration, or a deliberate host embedding of this code. It is not Wasm contract storage and must never be exposed as a contract entry point.

## Contract wiring

Embed the standard messages in the application's execute/query schemas. Delegate a funding-management call with:

```rust,ignore
execute_async(&info, async_message, configured_fee_denom)
```

Delegate a query using the current contract environment:

```rust,ignore
query_funding(deps, &env, async_query)
```

These return `Response<SchedulerMsg>` and authoritative host query results through `SchedulerQuery`. Use `Deps<SchedulerQuery>` in the contract. Discover the fee denomination through `SchedulerQuery::FundingConfig` or a deployment configuration validated against that query. The host independently validates every coin denomination.

The test contract in [tests/support/mod.rs](tests/support/mod.rs) demonstrates registration, an explicit split of business funds and initial gas funds, and the standard message routing. Its deliberately dishonest `ClaimFunds` and `FundThenFail` paths are adversarial fixtures, not recommended contract APIs.

`RegisterTask` currently registers **only a funding record**. It carries no executable continuation yet; `Runnable` means the funding state permits execution, not that a task will actually be polled. Its host response data contains the allocated task ID as JSON. Contract start APIs should use submessage reply handling to return that ID to their callers; do not assume a child message's response data automatically becomes the contract's execute response.

## Custody and trust boundary

The chain adapter must:

1. Derive the owning address from authenticated executing-contract context and reject direct account submissions of scheduler messages.
2. Supply a scheduler module account that ordinary accounts/contracts cannot sign for or impersonate.
3. Execute keeper state changes, actual bank transfers, and downstream contract messages inside one transaction cache. Propagate errors to that cache boundary.
4. Implement the transfer callback as an actual bank debit and credit. A successful no-op transfer is not an adapter.
5. Store keeper records in a host-only namespace and supply consensus-controlled configuration.

`actor` and `contributor` are assertions made by the owning contract, not independently authenticated transaction signers. SDK routing forwards `MessageInfo.sender`; the owning contract is trusted to enforce its advertised controller policy. A malicious owner contract can lie about that identity, but cannot select another owning contract in a scheduler message. This is contract-controlled escrow, not an independent beneficiary vault.

The host debits the owning contract's actual balance, so a JSON amount cannot mint credit. It cannot distinguish attached user coins from treasury coins once they are in that balance. The contract must enforce its business/gas split and treasury sponsorship authorization. Plain bank transfers create no scheduler credit.

Deposits do not resume tasks or change refund ownership. Contributors fund the existing budget; unused execution funds go to the recorded refund address on cancellation. Funding transactions pay their ordinary transaction fee separately. Cancellation is allowed after logical expiry to reclaim funds while automatic cleanup is not yet implemented. Top-ups and resumption are rejected at the exact expiry height.

Configuration's fee denomination and custody account must remain fixed for the lifetime of these records. Gas price can change under host policy; funding queries recalculate the maximum attempt charge and resume checks the task's price cap. `gas_per_poll` denotes the future total metered attempt allocation; this increment does not meter, reserve, or charge execution gas.

## Validation and remaining runtime work

```sh
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all -- --check
cargo build -p native-async --target wasm32-unknown-unknown --locked
```

The default feature set contains only wire types and contract helpers. Enable `host` to run the funding integration tests. cw-multi-test verifies transactional behavior in simulation; it does not establish real VM gas use, block latency, or chain deployment compatibility.

Remaining work: executable registration and poll ABI, bounded block queues, durable inboxes/timers, fee reservation and settlement outside poll rollback, completion/failure refunds, storage leases and admission limits, retained-result pruning, migration gating, and a pinned wasmd/SDK integration. Queries intentionally omit reserved gas, cumulative fees, and storage lease balances until those accounting components exist. Do not use this reference keeper on a production chain before those limits and economics are implemented.
