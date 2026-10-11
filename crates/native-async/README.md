# Native async ABI and reference host runtime

This crate implements the contract funding protocol, executable task ABI, and a storage-backed reference scheduler. Contracts using these custom messages require a chain adapter; unmodified wasmd does not understand them.

Implemented:

- Standard `AsyncExecuteMsg::{FundTask, ResumeTask, CancelTask}` and `AsyncQueryMsg::{FundingConfig, TaskFunding}`.
- Contract helpers that forward attached native coins and the actual `MessageInfo.sender`.
- Host-owned, contract-scoped task funding records and monotonic IDs.
- Registration, positive deposits, checked balance arithmetic, explicit resume, cancellation, and refunds.
- Logical expiry checks, ceiling-rounded attempt prices, saved waiting/runnable state, and refund/control isolation from sponsors.
- Transactional tests exercising bank transfers, cross-contract isolation, insufficient balances, overflow, and downstream rollback.

The optional `host` feature is a **reference implementation**, using CosmWasm's storage interface for a portable test harness. A production Cosmos SDK keeper will need equivalent Go state transitions and bank integration, or a deliberate host embedding of this code. It is not Wasm contract storage and must never be exposed as a contract entry point.

## Executable runtime

`RuntimeMsg::Register` carries `TaskProgram`, the existing funding fields, and a prepaid byte allocation. The host derives the contract's code checksum. `RuntimeMsg::Funding` wraps the standard funding messages; `RuntimeMsg::Deliver` forwards the owner contract's authenticated callback sender. Registration returns a JSON task ID in response data. `RuntimeQuery::Task` returns the shared public `TaskResponse`: execution state, typed status (including pause reason and resume state), code identity, attempt height, lease and allocation metadata. Optional fields serialize as null. Terminal output and error have separate fields; internal queue indexes and financial bookkeeping are excluded. This intentionally corrects the pre-release query ABI, which previously serialized incompatible backend storage records. `RuntimeQuery::Funding` exposes the standard funding queries, including reserved funds, cumulative execution fees, storage allocation, and lease payment.

The private sudo ABI is `TaskSudoMsg::PollTask(PollRequest)`. Contract sudo returns `PollResponse` encoded in response data, with **no outer dispatchable messages**. `PollResponse.messages` are dispatched by the host only after validating and staging the new continuation/wait. The first wait sequence is 1. A single poll can return Pending, Yield, Complete, or Fail; resolutions include success, application error, timeout, and height timer. Shared JSON fixtures under `fixtures/native-async/v1` are typed-roundtripped by Rust and consumed by the Go integration.

With `host` enabled, `runtime::Runtime` provides:

- Persistent height indexes for readiness, expiry, and pruning; FIFO task queues per contract and a persistent contract round-robin ring. No sleeping-task scan is used. A gas-deferred head retains priority in the next block; the implementation conservatively stops dispatch for the remainder of the current block.
- Independent visit, promotion, expiration, and pruning limits plus a separately reserved maintenance gas allocation. The application must partition ordinary transaction gas, async `block_gas`, and `maintenance_gas` before transaction admission.
- At most one attempt per task/block. Registration, yields, and scheduler-produced callbacks become eligible at H+1. Ordinary callbacks to existing tasks can run at H.
- Exact operation deadlines, authenticated bounded durable inboxes, preserved accepted outcomes after deadline, and logical expiry on access even when cleanup is delayed.
- Full attempt escrow reservation before guest dispatch, actual trusted host-meter settlement, unused reservation release, and fees that survive guest rollback. `PollHost` returns metered task failures separately from node errors; node errors abort the whole block cache.
- Child storage caches for guest writes, scheduler state, bank effects, and outbound calls. Trap, out-of-gas, invalid outcome, and failed messages restore the prior continuation/inbox. Application Fail commits prior writes and becomes terminal.
- Staged waits for immediate service callbacks. In-flight cancellation/resumption is rejected; atomic third-party deposits survive settlement without granting retry authority.
- Prepaid byte/height leases with controlled monotonic growth, admission limits, completion/failure/cancel/expiry refunds, bounded retained-record pruning, and active-task migration gating. IDs are monotonic per owning contract and never reused after pruning.

`PollHost` is an explicit trusted adapter contract: all user/bank effects must use the supplied transactional storage; gas must come from the host meter; checksum must come from the actual contract; callbacks must pass the actual scheduler phase. A no-op transfer or process-local user side effect is not a valid adapter. Route **all** management of executable records through `Runtime`, rather than bypassing it with the legacy funding-only `Keeper`.

Byte allocation reserves maximum-width mutable accounting/counters, the largest lifecycle/pause and queue-key encodings, a bounded 1024-byte index allowance, and worst-case escaped JSON inbox capacity. Top-ups, resume, and policy updates recheck voluntary changes inside their transaction cache; mandatory fee settlement and pause recording use prepaid metadata headroom and cannot fail a quota check. Empty contract ring/membership records are removed during terminal cleanup before storage allocation can be released. The configured host/maintenance gas costs and allocation allowance are reference policy parameters, not production calibration. Migration remains conservatively blocked until expired records have completed their bounded terminal transition.

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

The legacy `SchedulerMsg::RegisterTask` still registers **only a funding record**. Use `RuntimeMsg::Register` for executable tasks. Both return the allocated task ID as JSON. Contract start APIs should use submessage reply handling to return that ID to their callers; do not assume a child message's response data automatically becomes the contract's execute response.

## Custody and trust boundary

The chain adapter must:

1. Derive the owning address from authenticated executing-contract context and reject direct account submissions of scheduler messages.
2. Supply a scheduler module account that ordinary accounts/contracts cannot sign for or impersonate.
3. Execute keeper state changes, actual bank transfers, and downstream contract messages inside one transaction cache. Propagate errors to that cache boundary.
4. Implement the transfer callback as an actual bank debit and credit. A successful no-op transfer is not an adapter.
5. Store keeper records in a host-only namespace and supply consensus-controlled configuration.

`actor` and `contributor` are assertions made by the owning contract, not independently authenticated transaction signers. SDK routing forwards `MessageInfo.sender`; the owning contract is trusted to enforce its advertised controller policy. A malicious owner contract can lie about that identity, but cannot select another owning contract in a scheduler message. This is contract-controlled escrow, not an independent beneficiary vault.

The host debits the owning contract's actual balance, so a JSON amount cannot mint credit. It cannot distinguish attached user coins from treasury coins once they are in that balance. The contract must enforce its business/gas split and treasury sponsorship authorization. Plain bank transfers create no scheduler credit.

Deposits do not resume tasks or change refund ownership. Contributors fund the existing budget; unused execution funds go to the recorded refund address on termination. Funding transactions pay their ordinary transaction fee separately. Cancellation is allowed after logical expiry to reclaim funds before bounded automatic cleanup. Top-ups and resumption are rejected at the exact expiry height.

Configuration's fee denomination and custody account must remain fixed for the lifetime of these records. Gas price can change under host policy; funding queries recalculate the maximum attempt charge and resume checks the task's price cap. `gas_per_poll` denotes the total metered attempt allocation, including configured host overhead.

## Validation and remaining runtime work

```sh
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all -- --check
cargo build -p native-async --target wasm32-unknown-unknown --locked
```

The default feature set contains only wire types and contract helpers. Enable `host` to run the funding integration tests. cw-multi-test verifies transactional behavior in simulation; it does not establish real VM gas use, block latency, or chain deployment compatibility.

Reference tests exercise the storage-backed scheduler, bank adapter, immediate callbacks, rollback, exact boundaries, fairness, and lifecycle accounting. They do not establish real Wasm VM gas conversion, cold/warm validator equivalence, state sync, sustained overload progress, or production storage/gas calibration. Those require pinned chain integration and application-level evidence. Multiple simultaneous subscriptions and reserved per-block service remain outside this version.

## Controlled lease and budget management

`RuntimeMsg::ExtendLease` permits the controller to increase allocated bytes,
expiry, or both while the task is live, unexpired, and outside a poll. Both values
are monotonic; an identical lease is rejected. Growth prepays incremental rent
from available execution escrow at the current byte-block price, without another
cleanup fee. Insufficient escrow or capacity rejects the entire change. Top up
explicitly before extending if necessary. Operation deadlines and readiness are
unchanged, and the new expiry governs retention and pruning.

`RuntimeMsg::UpdateBudget` permits the controller to change the poll cap, maximum
gas price, and validated refund recipient under the same lifecycle restrictions.
Identical budget updates are accepted. This preserves prepaid lease and accumulated
fees and never resumes a paused task or bypasses a wait. Available escrow may be
below the new cap; explicit resume checks price and affordability. Sponsor deposits
confer no management authority. SDK `extend_lease` and `update_budget` forward
`MessageInfo.sender` and reject all attached funds; executable management always
goes through `Runtime`, never the funding-only keeper.
