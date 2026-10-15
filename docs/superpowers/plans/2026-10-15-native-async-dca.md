# Native Async DCA Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver a runnable local DCA example that executes finite scheduled trades, separates business funds from scheduler gas, and recovers without duplicating committed trades.

**Architecture:** Two contracts share a small example-only wire protocol. A generated native workflow schedules swaps; self-execute, exchange transfer, authenticated callback and a reply guard settle each slice atomically. A genuine reference-runtime adapter tests transactions in Rust, while pinned wasmd tests establish real-Wasm behavior and restart evidence.

**Tech Stack:** Rust 1.99.0, existing locked CosmWasm 2.x/cw-multi-test dependencies, durable-macro/runtime, native-async, Binaryen 123, Go 1.23.6, wasmd 0.55.1, Cosmos SDK 0.50.12, wasmvm 2.2.4; shell and Go for local scenarios.

**Spec:** [Approved native async examples design](../specs/2026-10-15-native-async-examples-design.md).

## Global Constraints

- The agreed suite is DCA first, milestone escrow and sealed-bid auction later; only DCA is implemented by this plan.
- Use native coins only, distinct input/output denominations, positive slice amount/minimum output/interval, and a slice count in `1..=128`.
- Bound successful result payloads to 2048 bytes.
- The synchronous fixture's operation deadline is dispatch height plus one; every wait due height is strictly less than immutable task expiry.
- First target is registration height plus interval; subsequent targets are previous committed fill height plus interval; never use `SleepBlocks(0)` or catch-up batches.
- Keep business principal, scheduler gas and ordinary transaction fees separate; attached principal plus initial gas must match exactly, including a shared denomination.
- Funding does not grant control or resume a task. Owner, beneficiary, exchange, expiry and gas refund owner are immutable; only poll cap/price ceiling can change.
- No compiler/runtime ABI changes, lease extension, arbitrary futures, IBC adapter, production deployment, runtime repair, merge, or new formal/state-sync-network claim.
- N1/N2/R1 remain open in the reviewed runtime. Demo minimum base price must be positive; use validated addresses and Go's stricter wait boundary.
- Real gas claims require the pinned Go host. Rust logical gas, in-memory reconstruction, database reopen, and actual process restart are distinct evidence levels.
- Preserve the dirty original checkout and unrelated PR branches. Stop and report if a runtime defect or new runtime interface blocks this design.

## Review Focus

1. Wide integer products and rounding dust must reject zero/unrepresentable outputs without spending coins (Task 2).
2. Host task IDs need not equal local order IDs; timer sequence numbers are not slice indices, and different contracts can reuse local IDs (Tasks 4 and 5).
3. Missing, malformed, cross-order or replayed reply payloads must not bind another task or confirm an absent fill (Tasks 4 and 5).
4. Escaped or maximum-width result fields and the 2048-byte limit must not bypass allocation/receipt checks (Tasks 1, 4 and 5).
5. A lost RPC response after broadcast must not cause a second transaction submission; journaled hashes and confirmed state decide the outcome (Task 7).

## Baseline, execution and ownership

The managed workspace is `/Users/zakimanian/.codex/worktrees/287f/cosmwasm-durable-async`, initially detached at design commit `f4aef17607e725cc62ccb6033183d5d33aae0879` atop PR #5 `9ec038632388343738c72b60e5512d29d2669e66`. Do not create another worktree or switch this one onto an unrelated PR. Branch naming/push can use the app's native controls at handoff; no remote publication is part of this request.

Recommended execution is task agents with independent per-task reviews, followed by a whole-change review. Use `gpt-6.1-sol` with high reasoning for contract/harness implementers, medium for bounded protocol/docs work, and `gpt-6-astra` with high reasoning for custody/atomicity review if those models remain available. Reassess rather than claiming a primary-model switch. Assign one owner to workspace manifests and the lockfile; schedule their edits to avoid collisions.

Dependency order: Task 1 → Tasks 2 and 3 independently → Task 4 → Task 5 → Task 6 → Task 7. Task 7's journal tests/docs draft can proceed alongside Task 6 once the public protocol is fixed. Each task gets spec-compliance and code-quality review before its dependents rely on it; controller verifies agent results and runs final checks.

- [ ] Record current PR heads, `git status --short`, Rust/Go/tool versions, and a fresh isolated `CARGO_TARGET_DIR` created with `mktemp -d`.
- [ ] Run `cargo test --workspace --all-features --locked`, `cargo fmt --all -- --check`, and `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` before product edits. Record baseline failures separately; do not weaken tests to conceal them.

Only this plan/spec are written at planning time. No listed implementation/test result below is already achieved.

## File map

| Path | Responsibility |
| --- | --- |
| `contracts/dca-protocol/{Cargo.toml,src/lib.rs,tests/wire.rs}` | Example-only shared request, receipt and callback DTOs; no contract entrypoints |
| `contracts/mock-exchange/{Cargo.toml,src/lib.rs,src/contract.rs,src/state.rs,src/error.rs}` | Immutable fixed-ratio exchange fixture |
| `contracts/mock-exchange/tests/swaps.rs` | Atomic exchange and callback tests |
| `contracts/native-dca/{Cargo.toml,src/lib.rs,src/msg.rs,src/state.rs,src/error.rs}` | DCA schema, persisted identities/accounting and entrypoint exports |
| `contracts/native-dca/src/{contract.rs,workflow.rs,settlement.rs}` | Public management/queries; generated workflow; atomic slice settlement |
| `contracts/native-dca/tests/support/{mod.rs,runtime_module.rs,poll_host.rs,fixtures.rs}` | Example-local transactional reference runtime adapter and helpers |
| `contracts/native-dca/tests/{host_adapter.rs,admission.rs,workflow.rs,lifecycle.rs}` | Adapter regression, admission, atomicity, accounting and recovery |
| `contracts/native-dca/examples/dca_fault.rs` | Wasm-only test counterparty modes for failed/missing/invalid callbacks; never the tutorial exchange |
| `scripts/wasmd-normalize.sh` and `contracts/native-dca/build-wasmd.sh` | Shared existing normalization policy and DCA artifact builder |
| `integration/wasmd/dca_acceptance_test.go` | Real-VM signed transactions, balances, host outcomes and deterministic database reopen |
| `integration/wasmd/{dca_demo_test.go,dca_demo_journal_test.go}` | Opt-in single-node scenarios and broadcast journal tests |
| `scripts/native-dca-demo.sh`, `contracts/native-dca/README.md`, `contracts/mock-exchange/README.md` | Reproducible local tutorial and fixture limitations |
| `docs/reviews/2026-10-15-native-dca-verification.md` | Actual final commands, artifact identities and evidence boundaries |

Existing files touched narrowly: root `Cargo.toml`/`Cargo.lock`, `README.md`, `contracts/native-fulfill/build-wasmd.sh`, `integration/wasmd/test-upstream.sh`, and `integration/wasmd/README.md`. Do not rewrite existing fulfillment contracts or scheduler code.

### Task 1: Freeze the example-only wire contract

**Files:** Create `contracts/dca-protocol/{Cargo.toml,src/lib.rs,tests/wire.rs}`; update root workspace members/lockfile. Package name `dca-example-protocol`.

**Interfaces:** Export `MAX_SLICES: u32 = 128`, `MAX_RESULT_BYTES: u32 = 2048`, existing `durable_runtime::Correlation`, and these owned serializable DTOs. Slice indices are zero-based; `correlation.workflow_id` is always the host task ID.

```rust
SwapRequest { order_id: u64, slice_index: u32, correlation: Correlation,
    input: Coin, output_denom: String, minimum_output: Uint128, beneficiary: String }
SwapReceipt { order_id: u64, slice_index: u32, correlation: Correlation,
    input: Coin, output: Coin, beneficiary: String }
ExchangeConfig { input_denom: String, output_denom: String,
    numerator: Uint128, denominator: Uint128 }
ExchangeExecuteMsg::Swap(SwapRequest)
DcaCallbackMsg::SwapResult { receipt: SwapReceipt }
ExchangeQueryMsg::{Config {}, Receipt { dca: String, order_id: u64, slice_index: u32 }}
```

All public enums use `#[serde(rename_all = "snake_case")]`. Exchange `Config` returns `ExchangeConfig`; `Receipt` returns `Option<SwapReceipt>`. No callback-address field exists in a request. Both contracts depend on this rlib, never on each other's Wasm entrypoints. Dependencies remain existing CosmWasm/serde/durable-runtime only.

- [ ] Add `wire_roundtrip_preserves_distinct_ids`, `callback_has_exact_snake_case_envelope`, and `receipt_size_counts_json_escaping` in `tests/wire.rs`.

```rust
assert_eq!(decoded.order_id, 7);
assert_eq!(decoded.correlation.workflow_id, 41);
assert_eq!(decoded.slice_index, 0);
assert_eq!(decoded.correlation.wait_sequence, 2);
assert!(json.starts_with("{\"swap_result\":{\"receipt\":"));
assert_eq!(to_json_binary(&receipt)?.len(), encoded_len); // count bytes, not chars
```

- [ ] Run `cargo test -p dca-example-protocol --test wire`; verify the new tests fail against missing/stub DTOs, not an environment failure.
- [ ] Implement the DTOs/constants and pin the actual callback envelope in a literal fixture. Add boundary payload fixtures of exactly 2048 and 2049 encoded bytes for later consumers.
- [ ] Run `cargo test -p dca-example-protocol --locked` and format checks; require all cases pass, then commit only protocol/workspace files as `feat: define DCA example wire protocol`.

### Task 2: Build the atomic fixed-price exchange fixture

**Files:** Create the listed `contracts/mock-exchange` files and tests; add workspace member/lockfile through the manifest owner. Package name `mock-exchange`, cdylib+rlib.

**Interfaces:** Consume Task 1 DTOs. `InstantiateMsg = ExchangeConfig`. Entry points `instantiate(DepsMut, Env, MessageInfo, ExchangeConfig) -> Result<Response, ContractError>`, `execute(DepsMut, Env, MessageInfo, ExchangeExecuteMsg) -> Result<Response, ContractError>`, `query(Deps, Env, ExchangeQueryMsg) -> StdResult<Binary>`. Internal `quote(input: Uint128, numerator: Uint128, denominator: Uint128) -> Result<Uint128, ContractError>`.

- [ ] Write `swaps.rs` using cw-multi-test plus a minimal authenticated callback receiver. Pin these cases before implementing exchange effects:

```rust
assert_eq!(quote(10u128.into(), 3u128.into(), 2u128.into())?, Uint128::new(15));
assert!(quote(1u128.into(), 1u128.into(), 3u128.into()).is_err());
assert!(quote(Uint128::MAX, Uint128::MAX, Uint128::one()).is_err());
assert_eq!(beneficiary_delta, coin(15, "uout")); // input=10, min=14
assert_eq!(callback_sender, exchange_address);
assert_eq!(balances_after_rejected_callback, balances_before);
assert_eq!(receipt_after_rejected_callback, None);
```

- [ ] Run `cargo test -p mock-exchange --test swaps`; observe failures for missing swap behavior.
- [ ] Implement immutable validated denominations/positive ratio, exact attached input validation, wide multiplication/floor division/checked narrowing, liquidity check, and duplicate receipt key `(actual sender, order_id, slice_index)`. Reject unexpected instantiate funds; prefund liquidity via ordinary bank transfer.
- [ ] Emit beneficiary `BankMsg::Send` before callback to actual `info.sender`. Save the receipt inside the same transaction; do not swallow bank/callback errors. Bound receipt encoding to 2048 bytes. No rate updates, fee collection or withdrawal API.
- [ ] Add min-output, invalid denom/address, zero/extra/duplicate funds, insufficient liquidity, duplicate swap and distinct-requester same-ID tests. Require rejected transactions preserve bank balances and receipts.
- [ ] Run `cargo test -p mock-exchange --locked` and clippy for that package; commit as `feat: add atomic mock exchange for async examples`.

### Task 3: Add a genuine reference-runtime contract test adapter

**Files:** Create native-dca package manifest and `tests/support/*`, `tests/host_adapter.rs`; add its workspace member/lockfile through the manifest owner. Minimal library placeholder only until Task 4. Package name `native-dca`, cdylib+rlib. Dev dependencies use existing locked cw-multi-test and `native-async` with `host` enabled.

**Interfaces:** `RuntimeModule` implements cw-multi-test `Module` with `ExecT = RuntimeMsg`, `QueryT = RuntimeQuery`, `SudoT = RunBlock`. `RouterPollHost<'a, ExecC, QueryC>` implements the existing `PollHost` trait using shared router/api/block references and the exact supplied transactional storage. `Suite::new() -> Suite`, `Suite::block(height: u64) -> AnyResult<BlockReport>`, and `Suite::reconstruct() -> AnyResult<()>` are test-only conveniences, not product APIs.

The existing `crates/native-async/tests/support/mod.rs` is a funding-only adapter and is insufficient. Reuse its router/bank patterns, not its `SchedulerMsg` interface. No production scheduler change belongs in this task.

- [ ] Add a minimal fixture contract that returns a pending wait plus nested self-message/callback. Tests `native_adapter_delivers_at_next_block`, `native_adapter_rolls_back_nested_failure`, and `native_adapter_reconstructs_same_state` must fail before the adapter routes real calls.

```rust
assert_eq!(report.polls, 1);
assert!(task.inbox.is_some());
assert_eq!(task.last_attempt, Some(height));
assert_eq!(same_height_second_run.polls, 0);
assert_eq!(balances_after_failed_dispatch, balances_before);
assert_eq!(state_after_reconstruct, state_before_reconstruct);
```

- [ ] Run `cargo test -p native-dca --test host_adapter --all-features`; confirm semantic failures using the stub adapter.
- [ ] Route custom execute/query to genuine `Runtime::{execute,query}` and `RunBlock` to `Runtime::run_block`. Reject direct noncontract custom-message senders using contract-info queries.
- [ ] Implement `PollHost::poll` via `router.sudo` with `WasmSudo::new(Addr::unchecked(contract), &TaskSudoMsg::PollTask(request.clone()))?.into()`, decode response data as `PollResponse`; implement dispatch with `CosmosMsg<Empty>::change_custom` and `router.execute`, and bank transfers through the same router/storage.
- [ ] During dispatch, save/restore a namespaced storage marker so nested custom `Deliver` uses `scheduler_phase=true`. Clean it up on success and failure. Supply immutable fixture checksums rebuilt on reconstruction; explicitly exclude real checksum/migration claims.
- [ ] Use `app.init_modules` to invoke `router.custom.sudo(api, storage, router, &block, RunBlock)` with a cloned block; `App::sudo` does not support custom module sudo. Rely on `Runtime::run_block`'s full-storage/per-attempt caches, never an uncached process-local bank/contract state map.
- [ ] Document a positive-floor, fixed logical gas schedule and test fee conservation using that schedule. cw-multi-test cannot measure SDK/VM gas. Require nested errors map to task pause reasons while infrastructure/test failures remain visible.
- [ ] Rerun the adapter tests and reference runtime suite (`cargo test -p native-async --features host --test runtime --locked`); commit as `test: add transactional native runtime adapter for DCA`.

### Task 4: Implement order admission, custody and management

**Files:** Create native-dca `src/{lib.rs,msg.rs,state.rs,error.rs,contract.rs}` and `tests/admission.rs`; update `tests/support/mod.rs` to deploy both contracts. No scheduling behavior is needed to pass admission tests.

**Interfaces:** Freeze these before Task 5. Entry points match native-fulfill's custom types: instantiate/execute/reply return `Result<Response<RuntimeMsg>, ContractError>` using `DepsMut<RuntimeQuery>`; query takes `Deps<RuntimeQuery>` and returns `StdResult<Binary>`.

```rust
InstantiateMsg { exchange: String, fee_denom: String }
CreateOrderRequest { input_per_slice: Coin, slice_count: u32, minimum_output: Uint128,
    interval_blocks: u64, beneficiary: String, gas: Coin, gas_per_poll: u64,
    max_gas_price: Decimal, expires_at: u64, allocated_bytes: u64 }
ExecuteMsg::{CreateOrder(CreateOrderRequest), Async(AsyncExecuteMsg),
    UpdateBudget { order_id: u64, gas_per_poll: u64, max_gas_price: Decimal },
    ReclaimPrincipal { order_id: u64 },
    DispatchSlice { order_id: u64, slice_index: u32, correlation: Correlation },
    SwapResult { receipt: SwapReceipt }}
QueryMsg::{Order { order_id: u64 }, Fill { order_id: u64, slice_index: u32 },
    Task { order_id: u64 }, Async(AsyncQueryMsg)}
CreateOrderResponse { order_id: u64, task_id: u64 }
```

All enums use snake case. `SwapResult` wire encoding must equal `DcaCallbackMsg`. Freeze `OrderRecord` fields as `order_id: u64`, `task_id: Option<u64>`, `owner/beneficiary/exchange: Addr`, `input_per_slice: Coin`, `output_denom: String`, `slice_count/filled_count: u32`, `minimum_output/initial_principal/remaining/reclaimed/spent/received: Uint128`, `interval_blocks/registration_height/expires_at: u64`, `pending: Option<PendingDispatch>`, and `last_fill_height: Option<u64>`. Here `received` is cumulative output delivered directly to the beneficiary, not contract-held output. `PendingDispatch` stores complete expected `SwapRequest`, `dispatch_height: u64`, and `deadline: u64`; `FillRecord` stores `receipt: SwapReceipt` and `height: u64`. `Config` stores validated exchange/fee/input/output settings. Use disjoint storage keys and checked monotonic order IDs.

Export `load_order(&dyn Storage, u64) -> StdResult<OrderRecord>`, `save_order(&mut dyn Storage, &OrderRecord) -> StdResult<()>`, `load_fill(&dyn Storage, order_id: u64, slice_index: u32) -> StdResult<Option<FillRecord>>`, and `save_fill(&mut dyn Storage, &FillRecord) -> StdResult<()>` (keys come from its receipt). These are the storage interfaces for Task 5; no business logic belongs in DTO serialization. The allocation preflight must include SDK metadata and `inbox_reserve(2048)` plus a conservative serialized-state allowance backed by maximum-width checkpoint tests; record that chosen allowance and its derivation, not an unexplained magic deposit.

- [ ] Add admission tests for 0/1/128/129 slices, all zero fields, checked total-principal overflow, invalid beneficiary, fee/exchange mismatch, exact funds split, duplicate/excess funds, and allocation below the required continuation+metadata+inbox reserve.
- [ ] Add `registration_reply_uses_host_id_not_reply_tag`: pre-seed a distinct next local order ID, create order 7 with host task 1, and assert returned IDs and stored ownership. Reject missing/malformed/cross-order reply payloads and rebinding a task.
- [ ] Run `cargo test -p native-dca --test admission --all-features`; observe failing assertions before implementing admission.
- [ ] Implement create/register atomically. Discover the exchange's immutable config and host fee config; validate settings. Send only initial gas to host, with control/refund derived from actual owner. Use register reply ID 1 and `RegisterReplyPayload { order_id }`; parse host response data as raw JSON `u64`, not Wasm protobuf or reply ID.
- [ ] Implement standard funding forwarding, owner-only budget updates with stored owner as refund, and owner-only principal reclaim after terminal status or immutable expiry. Track `OrderRecord.reclaimed`; reject empty/repeated reclaim and unexpected attached funds on nonfunding calls. Preserve sponsor ability to fund, not manage.
- [ ] Implement `OrderView { order: OrderRecord, host: HostTaskView }`, where `HostTaskView = Present(TaskResponse) | Unavailable { error: String, lease_expired: bool }`. The ABI does not type a pruned-record error: do not claim arbitrary failures prove pruning/completion. `Task` propagates host query errors; `Fill` returns `Option<FillRecord>`.
- [ ] Add tests for gas denom equal to input and equal to output, two owners, uncredited plain transfers, unauthorized management/reclaim, failed bank reclaim rollback, post-expiry recovery with absent host record, and transient query failure before expiry. Assert principal conservation and no cross-order drain.
- [ ] Run admission/adapter/protocol tests, format and clippy; commit as `feat: add DCA order custody and scheduler management`.

### Task 5: Implement the durable workflow and atomic settlement

**Files:** Create `src/workflow.rs`, `src/settlement.rs`, `tests/{workflow.rs,lifecycle.rs}`, extend `src/contract.rs` routing and `tests/support/fixtures.rs` fault modes.

**Interfaces:** `#[durable_workflow(kind = "native_dca", version = 1)] pub async fn native_dca(ctx: WorkflowCtx, order_id: u64) -> Result<DcaOutcome, DcaWorkflowError>`; `DcaOutcome::{Filled, LeaseWindowExhausted}`. `DcaWorkflowError` serializes unexpected wait errors; no automatic retry loop. `SliceRequest { order_id: u64, slice_index: u32 }`; `SwapSlice: Operation<Request = SliceRequest, Output = SwapReceipt>` with kind `native_dca/swap_slice/v1`.

Settlement functions `dispatch_slice(deps, env, info, order_id, slice_index, correlation)` and `swap_result(deps, env, info, receipt)` return `Result<Response<RuntimeMsg>, ContractError>`. `dispatch_reply(deps, env, Reply)` has the same return type. `sudo(DepsMut, Env, TaskSudoMsg) -> Result<Response, ContractError>` alone calls `native::poll` with result limit 2048.

- [ ] Add failing `three_slices_have_minimum_interval_and_no_catchup`, `all_filled_precedes_host_complete`, and `cancel_after_receipt_does_not_refund_spent_slice` tests. With input 10, 3 slices and exchange ratio 3/2:

```rust
assert_eq!((order.filled_count, order.spent, order.remaining), (1, 10u128.into(), 20u128.into()));
assert_eq!(beneficiary_output_delta, Uint128::new(15));
assert!(task.inbox.is_some()); // same committed fill, not yet consumed
assert_eq!(reclaimed_after_cancel, Uint128::new(20));
assert_eq!(exchange_committed_fills, 1);
```

- [ ] Run `cargo test -p native-dca --test workflow --test lifecycle --all-features`; verify missing workflow/settlement behavior fails.
- [ ] Implement the generated loop with typed checkpoints. Read stored order progress, sleep only for a positive remaining interval, dispatch at most one slice per operation, and compute next due from actual fill height. Before preparing any wait, check overflow and strict due-before-expiry; complete `LeaseWindowExhausted` rather than submitting an invalid wait. Unexpected Timeout/Error terminates diagnostically without a replacement swap.
- [ ] `SwapSlice::prepare` reads immutable terms, saves exchange as resolver, uses height+1 deadline, and emits a funds-free self `DispatchSlice`. Pure `decode` must not mutate state or perform contextual authorization; the runtime calls it twice.
- [ ] Implement self-only dispatch: verify host task/order/slice/correlation against stored state, interval/expiry and no pending request, debit principal, record pending dispatch, and send exact input in exchange execute submessage with reply ID 2 and `DispatchReplyPayload { order_id, slice_index, correlation }`.
- [ ] Implement authenticated callback: validate sender, exact pending identities/coins/beneficiary, result size and minimum output; record fill/height and spent/output counters, clear pending, then forward actual sender in `RuntimeMsg::Deliver`. Any settlement inconsistency returns a contract error, never an application `Fail` after partial accounting.
- [ ] Dispatch reply verifies exact matching fill and cleared pending marker, otherwise returns error. Reject unknown reply IDs/payloads and any replay. Successful reply emits no new effects.
- [ ] Add transactional tests for omitted/duplicate/stale/wrong-sender receipts, wrong amounts/denoms/beneficiary, exact 2048/2049 payload boundary, failed bank send, failed host delivery and failed reply. All business writes/transfers and fixture receipts must roll back; compare gas charges separately.
- [ ] Add timer/operation equality at expiry, height overflow, insufficient liquidity recovery, bid pause/cap change, sponsor top-up without resume, explicit owner resume, two orders/owners/contract addresses, cancellation before/after fills, exact expiry and post-pruning reclaim. Assert `principal = spent + remaining + reclaimed` after every committed step and at most one fill per order/slice.
- [ ] Reconstruct stored runtime, bank and contract state before a timer and between receipt/consumption; assert identical next outcomes without duplicate transfers. Keep fixture code identity reconstruction explicit.
- [ ] Run all native-dca/mock-exchange/protocol tests plus workspace regressions; commit as `feat: execute durable DCA trades with atomic receipts`.

### Task 6: Establish pinned real-VM and database-reopen evidence

**Files:** Create `scripts/wasmd-normalize.sh`, `contracts/native-dca/build-wasmd.sh`, `examples/dca_fault.rs`, `integration/wasmd/dca_acceptance_test.go`; narrowly update native-fulfill builder and native-dca manifest for the fault artifact.

**Interfaces:** Shared builder shell functions `ensure_wasmd_binaryen` and `normalize_wasmd INPUT OUTPUT` preserve existing checksum-pinned Binaryen 123 and lowering flags. Respect the selected `CARGO_TARGET_DIR`. DCA builder produces `artifacts/native_dca.wasm`, `mock_exchange.wasm`, and `dca_fault.wasm`, printing SHA-256s. Fault modes are test-only omitted/invalid/duplicate callbacks and controlled delivery/reply rejection; the runnable tutorial always uses mock-exchange.

Go helpers local to the DCA test file: `newDCAAcceptanceApp(t)`, `deployDCA(t, h)`, `executeDCA(t, h, signer, contract, msg, coins)`, and `readDCAState(t, h, orderID)`; their concrete types follow existing `appHarness` and SDK interfaces. Support owner+sponsor and distinct input/output/gas balances rather than modifying unrelated tests' hard-coded task/address assumptions.

- [ ] Write `TestDCAAcceptanceAtomicReceipt`, `TestDCAAcceptanceFailureRollback`, and `TestDCAAcceptanceDiskReopen` first. Run the targeted suite and verify missing artifact/behavior failures are identified, not presented as passing or skipped coverage.
- [ ] Reuse current artifact normalization policy and patched VM build scripts. Build the old artifacts plus DCA artifacts; do not change upstream runtime semantics to make them pass.
- [ ] Reuse `openAcceptanceApp`, signed transaction construction and explicit FinalizeBlock/Commit. Query `ExperimentalConfig`: minimum price `0.001`, host reserve `70_000_000`, max poll gas `100_000_000`, and rent `1` per byte-block are uncalibrated harness values. Compute sufficient initial deposits from live lease and attempt quotes, not a guessed tiny coin count.
- [ ] Assert exact nested self-execute → exchange → bank → callback → host delivery → reply behavior, actual balances, pending inbox, no same-block second poll, consumed-gas charging on rollback, explicit top-up/resume, cancellation and principal reclaim. Use real host-metered gas to calculate ceiling-rounded fees, not Rust logical gas.
- [ ] For disk reopen: commit a fill with an unconsumed inbox, record order/fill/task/balances/app hash, close VM+app/database, reopen the same home with fresh keepers, then finalize the next block. Require no extra trade and unchanged pre-resume evidence. Include before-timer reconstruction and the immutable-expiry boundary.
- [ ] Run builders, create a fresh `NATIVE_ASYNC_GOPATH` with `mktemp -d` and set `NATIVE_ASYNC_GOCACHE` to its `build` child, then run `NATIVE_ASYNC_KEEP=1 bash integration/wasmd/test.sh`. Export its printed actual `GOWORK`, set `GOPATH=$NATIVE_ASYNC_GOPATH` and `GOCACHE=$NATIVE_ASYNC_GOCACHE`, then run from `integration/wasmd`: `GOTOOLCHAIN=go1.23.6 go test -tags upstream -count=1 -run '^TestDCAAcceptance' .` and the full suite. Only GOWORK is printed by the current bootstrap; retain the explicitly selected cache paths rather than guessing old temporary directories.
- [ ] Record passing/failing commands and artifact hashes; commit as `test: validate DCA atomicity and recovery in pinned wasmd` only after the required new cases pass. Report blockers honestly if the platform cannot run them.

### Task 7: Ship the local tutorial runner and complete verification

**Files:** Create `scripts/native-dca-demo.sh`, `integration/wasmd/{dca_demo_test.go,dca_demo_journal_test.go}`, both contract READMEs and the verification receipt; update root/integration READMEs and `test-upstream.sh` narrowly for a quoted optional `NATIVE_ASYNC_TEST_RUN` filter (default retains the full suite).

**Interfaces:** `scripts/native-dca-demo.sh happy|funding|liquidity|cancel|restart|all`. Internally invoke opt-in `TestNativeDCADemo` using `NATIVE_DCA_DEMO=1` and `NATIVE_DCA_SCENARIO`; use the current test executable as one child validator process. Reuse `state_sync_network_test.go`'s loopback/process/ABCI patterns, not its three-node topology or state-sync claims.

Journal interfaces within Go test utilities: `writeIntent(path string, tx []byte) (hash string, err error)`, `readJournal(path string) (TxJournal, error)`, `broadcastOnce(ctx context.Context, rpc DemoRPC, path string) (TxReceipt, error)`, and `recordResult(path string, receipt TxReceipt) error`. `DemoRPC` exposes CometBFT `BroadcastTxSync(context.Context, cmttypes.Tx) (*coretypes.ResultBroadcastTx, error)` and `Tx(context.Context, []byte, bool) (*coretypes.ResultTx, error)`. `TxJournal` stores `TxHex`, `Hash`, `Stage` and `Receipt *TxReceipt`; stages are `prepared`, `attempted`, `resolved`. `TxReceipt` stores `Hash`, `Outcome` (`unknown`, `check_tx_rejected`, `committed`), `CheckTxCode/DeliverTxCode *uint32`, and `Height *int64` so unknown is never silently zero.

Persist exact signed bytes/hash and sync+atomic-rename before submission; persist `attempted` before the RPC call. An existing attempted journal may query its hash but must never broadcast again. Refuse to overwrite an existing intent, validate hash against stored bytes on read, and preserve the original intent when writing result evidence. Do not persist/use non-test wallet keys.

- [ ] Write `TestDCADemoJournalNoRebroadcastAfterLostResponse`, `TestDCADemoJournalRejectsCorruptIntent`, `TestDCADemoRPCDeadline`, and `TestDCADemoCleanupOwnedChild`. Use fake transports/processes to assert:

```go
require.Equal(t, 1, broadcastCalls) // timeout after possible submission
require.Equal(t, originalHash, lookupHash)
require.Error(t, runErr)            // unresolved outcome stops scenario
require.True(t, unrelatedProcessStillAlive)
```

- [ ] Run `go test -tags upstream -run '^TestDCADemo(Journal|RPC|Cleanup)' .` in the prepared pinned workspace; confirm red assertions before implementing runner behavior.
- [ ] Implement one-validator local genesis with test assets and owner/sponsor identities. Refuse nonloopback RPC endpoints. Use per-RPC timeouts within bounded scenario deadlines, journal before submission, query the same hash after uncertain outcomes and stop without rebroadcast. Launch and clean up only owned child processes/temp paths.
- [ ] Implement the five scenarios with state, bank, fill and gas queries after each transition. Show business-filled progress separately from host completion, sponsor top-up without resume, and explicit owner actions. Replenish mock output liquidity for the recoverable liquidity scenario; immutable bad minimum-output orders require cancellation, not a price-admin backdoor.
- [ ] Make restart deterministic with a test-only gate before the next FinalizeBlock after the selected fill commit. Stop the owned child, restart the same home, release the gate, and verify the stored receipt is consumed without another trade. Do not infer this narrow window from polling a fast chain; do not change production scheduler code for the gate.
- [ ] Document first-run prerequisites/downloads, every command, illustrative output labeled as such, how to inspect task IDs/funding, exact min-interval semantics, immutable terms, unrecoverable timeouts, explicit principal reclaim and known runtime limits. Keep demo commands reproducible from repository root and explain stock wasmd is insufficient.
- [ ] Run `bash -n scripts/native-dca-demo.sh scripts/wasmd-normalize.sh contracts/native-dca/build-wasmd.sh`, journal tests, each scenario independently, and `scripts/native-dca-demo.sh all`. Verify child exit and retain small logs/receipts without retaining unnecessary build directories.
- [ ] Run final `cargo test --workspace --all-features --locked`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, both artifact builders, full pinned Go suite, and `git diff --check`.
- [ ] Independently review the full change, fix scoped findings with regression tests, then write the verification receipt with exact source/artifact identities, commands, results and untested boundaries. Do not claim state sync, Quint proof or runtime defects fixed. Commit as `docs: ship runnable native DCA scenarios and verification`.

## Acceptance and handoff

All seven tasks, their review gates, and the spec's eight validation groups must have concrete evidence before claiming the example is built. If a required test cannot run, describe the remaining gap rather than substituting a mock result. Leave runtime repairs and the next two example designs separate.

Final handoff includes the local commit(s), changed paths, reproducible demo command, test summary and known limitations. Do not push, open a PR, merge or deploy unless subsequently requested. This managed worktree is detached; use native branch creation/hand-off controls before any later publication.
