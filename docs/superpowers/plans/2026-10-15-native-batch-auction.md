# Native Batch Auction and Runtime Safety Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Repair the native runtime boundaries used by the examples and deliver a runnable, gas-bounded, multi-block auction with conserved principal and verified recovery.

**Architecture:** Repair explicit in-flight exclusion and wait admission without changing the guest wire ABI. A generated workflow processes a frozen commit/reveal book in bounded passes; application finalization unlocks separate atomic claim transactions. Real-Wasm tests, separate-process restart/state sync, and expanded Quint abstractions provide distinct evidence.

**Tech Stack:** Rust 1.99.0; locked CosmWasm std 2.3.5 and cw-multi-test 2.5.1; existing durable-macro/runtime and native-async; SHA-256 via already-locked sha2 0.10.9; Binaryen 123; Go 1.23.6; wasmd 0.55.1, Cosmos SDK 0.50.12, wasmvm 2.2.4; Quint 0.33.0 and the TypeScript runner.

**Spec:** [Approved Stage A specification](../specs/2026-10-15-native-batch-auction-design.md). The [suite roadmap](../specs/2026-10-15-native-async-showcase-suite-design.md) remains the source for later jobs, waitsets and IBC work; those stages are not silently implemented by this plan.

## Global Constraints

- `creation_height < C < R < E`; all pending waits require `poll_height < due_height < task.expires_at`.
- `Pmax = ceil(tick_count/B) + 2*ceil(max_bids/B) + 4`; admission requires `R + Pmax + 1 < E` with checked arithmetic.
- One auction per instance, auction ID `1`, no migration entry point, no chain-level admin in demos.
- `1 <= tick_count <= 256`, `max_bids` in `1..=256`, `batch_size B` in `1..=32`, query pages at most 32.
- Positive native-token supply/lot size, distinct sale/quote denominations, strictly increasing positive integer price grid.
- Quantities, supply, prices, coin liabilities and payouts fit `Uint128`; demand and pro-rata intermediates use checked `Uint256`.
- Exact attached funds only; keep principal, scheduler escrow and ordinary transaction fees separate even when denominations coincide.
- One commitment per bidder, no replacement, 32-byte digest/salt, bidder-only reveals, full refund for unrevealed bids.
- No claims before finalization; no finalization at E; no host-status-driven early principal release.
- After the first commitment, no cancellation or budget reduction. Sponsors gain no control; top-up never resumes.
- All calls preserve actual caller identity; resume/budget control and gas refund ownership remain with the seller.
- No public batch-advance route, unbounded scans, waitset ABI, IBC, new fairness policy, mainnet deployment, merge or automatic publication.
- Model passes are not implementation proofs; logical gas, compiled Wasm, database reopening, process restart and network state sync are separate evidence.

## Review Focus

1. A lost transaction response must trigger lookup of the original signed hash, never an automatic rebroadcast or new commitment (Task 9).
2. Generic funding/control forwarding must not bypass the auction's post-first-bid cancellation and nondecreasing-budget restrictions (Task 4).
3. Ordinary helper errors lower to application `Fail`, which commits writes unless the auction's sudo wrapper returns a transaction error (Task 5).
4. Restored snapshots must retain application claim flags/liabilities after host records are pruned, and cannot contain a transient in-flight marker (Tasks 1, 2, 7, 8).
5. A second payout failing must undo the first payout and the claim flag; alternate address spelling or JSON numeric encoding cannot redirect a claim or alter a commitment (Tasks 2, 3, 4, 6).

---

## Baseline and execution ownership

The documentation checkout is `/private/tmp/cosmwasm-async-suite.4ry5fN`, branch
`docs/async-showcase-suite`, at design commit
`e5a087fdd98eb700c15a999110d27adbdbfbcbb7` before this plan. Its code base is DCA
`5566d5bd3fd37a02fffc725c919f3980a1187b46`. Preserve the dirty original checkout
and the published DCA branch. At execution, use the worktree skill to verify
isolation and choose the implementation branch without discarding these docs.

Recommend task agents with independent task reviews and one whole-change review.
Use `gpt-6.1-sol` high reasoning for implementation, medium for bounded scripts/docs,
and `gpt-6-astra` high reasoning for custody/runtime/formal review if still
available. Reassess after evidence; do not claim a primary-model switch.

Dependency order: Tasks 1 then 2 own shared runtime files; Task 3 can proceed
independently. Task 4 depends on 2+3; Task 5 on 4; Task 6 on 5; Task 7 on 6;
Task 8 can proceed after 2+3 with final source correspondence checked after 5;
Task 9 follows 6+7+8. One controller owns Cargo manifests/lockfile changes and
shared Go helper integration. Do not have concurrent agents edit the same file.

- [ ] Record source/PR heads, `git status --short`, Rust/Go/Node/npm versions, and
  available disk space. Use fresh task-specific target/cache directories created
  with `mktemp -d`; do not reuse a target directory being written by another branch.
- [ ] Run baseline `cargo test --workspace --all-features --locked`,
  `cargo fmt --all -- --check`, and
  `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`.
  Record unrelated failures separately. Do not change source merely to suppress them.
- [ ] Build existing fulfill/DCA artifacts and bootstrap the pinned Go environment
  using the commands in Task 6. Preserve a fresh baseline log before runtime edits.

## File map and task boundaries

| Files | Responsibility |
| --- | --- |
| `crates/native-async/src/{runtime.rs,runtime_store.rs,host.rs}` | Private flight marker, safe top-ups, trusted resolver admission |
| `crates/native-async/tests/{runtime.rs,funding.rs,wait_admission.rs}` | Transactional runtime regressions and shared behavioral fixtures |
| `fixtures/native-async/wait-admission.json` | Cross-language acceptance cases, separate from encoding-only v1 fixtures |
| `integration/wasmd/{resolver_policy.go,wait_admission_test.go}` | Shared canonical resolver policy and parity tests |
| `integration/wasmd/{scheduler.go,snapshot_state.go,funding_callback_test.go}` | Narrow policy integration, restore and nested funding regressions |
| `contracts/native-batch-auction/src/{msg.rs,state.rs,error.rs,math.rs,commitment.rs}` | Protocol, storage, checked arithmetic and byte-exact commitments |
| `contracts/native-batch-auction/src/{contract.rs,admission.rs,claims.rs,queries.rs}` | Public routes, registration, funding/controls and principal claims |
| `contracts/native-batch-auction/src/{settlement.rs,workflow.rs}` | Bounded phase computation and generated native workflow |
| `contracts/native-batch-auction/tests/{math.rs,commitment.rs,admission.rs,claims.rs,workflow.rs,lifecycle.rs}` | Pure and transactional contract coverage |
| `contracts/native-batch-auction/tests/support/{mod.rs,runtime_module.rs,poll_host.rs,bank.rs}` | Auction fixture over genuine reference runtime; rejectable payouts |
| `contracts/native-batch-auction/{build-wasmd.sh,examples/auction_fault.rs}` | Normalized production and test-only fault artifacts |
| `integration/wasmd/{auction_acceptance_test.go,auction_restart_test.go,auction_state_sync_test.go}` | Real Wasm, committed database, new-process and snapshot-joiner evidence |
| `formal/quint/` | Imported pinned tooling, repaired native abstraction and auction model |
| `integration/wasmd/{auction_demo_test.go,auction_demo_node_test.go,auction_demo_journal_test.go}` | Loopback scenarios, owned process lifecycle, durable submission receipts |
| `scripts/{native-auction-demo.sh,native-auction-demo-test.sh,auction-commitment.mjs,auction-commitment.test.mjs}` | Runnable examples, smoke tests and independent client hash encoding |
| `contracts/native-batch-auction/README.md`, root/integration README, verification receipt | Developer usage and exact evidence boundaries |

Create only files needed by their owning task. Reuse existing normalized builders
and test infrastructure without turning this into a general harness rewrite.

### Task 1: Repair explicit in-flight control exclusion and nested funding

**Files:** Modify `crates/native-async/src/{runtime.rs,runtime_store.rs,host.rs}`,
`crates/native-async/tests/{runtime.rs,funding.rs}` and
`integration/wasmd/funding_callback_test.go`.

**Interfaces:** Preserve all public message shapes. Private helpers in
`runtime_store.rs`: `flight_key(contract: &str, id: u64) -> Vec<u8>` and
`is_in_flight(storage: &dyn Storage, contract: &str, id: u64) -> bool`.
Use reserved key prefix `na2/transient/flight/` and unambiguous identity encoding.
The marker is installed only in the per-attempt cache before `PollHost::poll`,
visible through nested caches, and removed before a successful cache commit.

- [ ] Add `zero_price_reentrant_controls_are_rejected` and
  `nested_funding_preserves_reserved_liability` to the existing reentrant host
  fixture, testing all four controls and Pending/Yield/Complete/Fail responses.

```rust
assert!(same_task_cancel.is_err());
assert!(same_task_resume.is_err());
assert!(same_task_extend.is_err());
assert!(same_task_budget.is_err());
assert!(authorized_other_task_management.is_ok());
// Successful nonterminal attempt; terminal cases check refund balances instead.
assert_eq!(funding_after.available, available_before + deposit - paid);
assert!(!storage_has_prefix(b"na2/transient/flight/"));
```

- [ ] Run `cargo test -p native-async --features host --test runtime --locked`
  and `--test funding`; retain the semantic failures at zero price and at the
  available-plus-reserved overflow boundary before changing production code.
- [ ] Add the marker checks to both `manage` and `update_policy`. Leave callback
  delivery and `FundTask` permitted. Preserve post-dispatch task/funding reloads.
  Change the keeper's deposit preflight to checked
  `available + reserved + deposit` before any transfer or write.
- [ ] Extend tests for positive price, successful top-up, failed-dispatch top-up
  rollback, OOG, host error, management after the attempt, and untouched other-task
  indexes/leases. Inspect the full committed store for leaked markers after every
  successful and failed run. Add corresponding Go tests without replacing its
  existing context-only guard.
- [ ] Run Rust runtime/funding tests and pinned Go
  `go test -tags upstream -count=1 -run 'Test.*(Funding|InFlight)' .` from
  `integration/wasmd`; require the new named cases pass, not skip.
- [ ] Commit the scoped change as
  `fix: separate async flight identity from gas reservations`.

### Task 2: Align wait admission and restore policy across hosts

**Files:** Modify `runtime.rs`, `crates/native-async/tests/runtime.rs`, Go
`scheduler.go`/`snapshot_state.go`; create `resolver_policy.go`,
`wait_admission_test.go`, Rust `tests/wait_admission.rs`, and the JSON fixture.
Existing DCA test adapter call site is
`contracts/native-dca/tests/support/runtime_module.rs`.

**Interfaces:** Change the Rust source API to
`Runtime::run_block(&self, api: &dyn Api, s: &mut dyn Storage, h: u64,
host: &mut impl PollHost) -> StdResult<BlockReport>`; thread the API through
`run_block_cached` and private `stage`. Go helper:
`validateResolver(raw string) (sdk.AccAddress, error)` parses using the configured
chain policy and rejects when `parsed.String() != raw`. No JSON ABI changes.

- [ ] Add a shared fixture with fields `name`, `height`, `expiry`, `kind`, `due`,
  `resolver`, `expected_accept`. For h=2/E=10, assert due 2,10,11 reject and 3,9
  accept for both timer and operation waits. Include canonical, mixed/upper-case,
  wrong-prefix, malformed and oversized addresses with the same configured prefix.

```rust
assert!(!accepted("due_equals_expiry"));
assert!(accepted("due_before_expiry"));
assert!(!accepted("noncanonical_resolver"));
assert_eq!(rejected_case.outbound_effects, 0);
assert_eq!(rejected_case.pause_reason, PauseReason::InvalidResponse);
```

- [ ] Run the new Rust test before implementation and confirm current lease and
  resolver acceptance disagrees with the fixture. Run the Go fixture to identify
  any canonical-spelling gap independently.
- [ ] Validate resolvers through `Api::addr_validate` plus exact textual equality;
  require strict lower and upper due bounds. Replace permissive runtime-test
  resolver labels with real validated addresses. Do not rewrite illustrative
  encoding-only fixture strings or silently normalize user input.
- [ ] Use the same Go helper in staging and restored operation-wait checks.
  Add restoration cases for invalid due/resolver and valid active v1 state. Keep
  configuration-specific address-length/prefix behavior explicit in fixture setup.
  A bad snapshot must fail closed; do not discard its offending wait to proceed.
- [ ] Run `cargo test --workspace --all-features --locked` and pinned Go
  `go test -tags upstream -count=1 -run 'Test.*(WaitAdmission|Snapshot|Restore)' .`.
  Review all changed `run_block` callers using `rg`; require all suites pass.
- [ ] Commit as `fix: align native wait admission and restore validation`.

### Task 3: Define auction protocol, checked allocation arithmetic and commitments

**Files:** Create the auction manifest, `src/{lib.rs,msg.rs,state.rs,error.rs,math.rs,commitment.rs}`,
`tests/{math.rs,commitment.rs}`, `fixtures/auction-commitments.json`,
`scripts/{auction-commitment.mjs,auction-commitment.test.mjs}`; update root Cargo
files through the controller.

**Interfaces:** Package `native-batch-auction`, cdylib+rlib. Use existing locked
dependencies plus `sha2 = "0.10.9"`; no new framework. `ContractError` wraps
`StdError`, checked overflow errors and fixed application validation categories.
All public enums use snake case; CosmWasm integer amounts serialize as strings.
Define `MAX_CHECKPOINT_BYTES: usize = 4096`, `MAX_RESULT_BYTES: u32 = 64`, and
`MIN_ALLOCATED_BYTES: u64 = native_async::METADATA_ALLOWANCE +
native_async::inbox_reserve(MAX_RESULT_BYTES) + 16384`. These are conservative
admission envelopes verified against generated states in Task 5, not gas estimates.

```rust
AuctionTerms { supply_lots: Uint128, lot_size: Uint128,
    reserve_price: Uint128, tick_size: Uint128, tick_count: u16,
    max_bids: u16, batch_size: u16,
    commit_end: u64, reveal_end: u64, expires_at: u64 }
InstantiateMsg { sale_denom: String, quote_denom: String, fee_denom: String }
CreateAuctionRequest { terms: AuctionTerms, gas: Coin, gas_per_poll: u64,
    max_gas_price: Decimal, allocated_bytes: u64 }
RevealBidRequest { quantity_lots: Uint128, price_tick: u16,
    beneficiary: String, salt: Binary }
ExecuteMsg::{CreateAuction(CreateAuctionRequest),
    CommitBid { digest: Binary, collateral: Uint128 }, RevealBid(RevealBidRequest),
    ClaimBid { bidder: String }, ClaimSeller {}, Abort {}, Async(AsyncExecuteMsg),
    UpdateBudget { gas_per_poll: u64, max_gas_price: Decimal }}
QueryMsg::{Auction {}, Bid { bidder: String },
    Bids { start_after: Option<u16>, limit: Option<u16> },
    Task {}, Async(AsyncQueryMsg)}
CreateAuctionResponse { auction_id: u64, task_id: u64 }
WorkflowInput { auction_id: u64 }
BusinessState::{Active, Finalized, Aborted}
SettlementPhase::{ScanPrice, AllocateBase, BuildClaims, Finalize, Done}
Clearing { price: Uint128, marginal_tick: Option<u16>,
    marginal_lots: Uint128, marginal_demand: Uint256 }
BidEntitlement { sale: Uint128, quote_refund: Uint128 }
SellerEntitlement { sale: Uint128, quote: Uint128 }
```

Freeze the shared persisted structures here so subsequent task agents do not
independently choose incompatible field names:

```rust
Config { seller: Addr, sale_denom: String, quote_denom: String, fee_denom: String }
Budget { gas_per_poll: u64, max_gas_price: Decimal }
ClaimTotals { bidder_sale: Uint128, bidder_quote: Uint128,
    seller_sale: Uint128, seller_quote: Uint128 }
RevealedBid { quantity_lots: Uint128, price_tick: u16, beneficiary: Addr }
AuctionRecord { auction_id: u64, task_id: Option<u64>, created_at: u64,
    terms: AuctionTerms, state: BusinessState, commit_count: u16,
    total_collateral: Uint128, total_demand: Uint256,
    settlement: SettlementProgress, claimed: ClaimTotals,
    seller_claimed: bool, budget: Budget }
BidRecord { index: u16, bidder: Addr, digest: Binary, collateral: Uint128,
    reveal: Option<RevealedBid>, allocated_lots: Uint128,
    entitlement: Option<BidEntitlement>, claimed: bool }
SettlementProgress { phase: SettlementPhase, cursor: u16,
    higher_demand: Uint256, clearing: Option<Clearing>,
    base_allocated: Uint128, residual: Uint128, final_allocated: Uint128,
    bidder_sale_total: Uint128, bidder_refund_total: Uint128, seller_quote: Uint128 }
```

Use prefix `auction/v1/`, fixed-width big-endian bid-index keys, a validated
bidder-to-index map, and separate tick buckets. Storage APIs:
`load_config(&dyn Storage) -> StdResult<Config>`;
`load_auction(&dyn Storage) -> StdResult<AuctionRecord>`;
`save_auction(&mut dyn Storage, &AuctionRecord) -> StdResult<()>`;
`load_bid(&dyn Storage, u16) -> StdResult<BidRecord>`;
`save_bid(&mut dyn Storage, &BidRecord) -> StdResult<()>`;
`bid_index(&dyn Storage, &Addr) -> StdResult<Option<u16>>`;
`load_demand(&dyn Storage, u16) -> StdResult<Uint256>` (missing means zero);
`save_demand(&mut dyn Storage, u16, Uint256) -> StdResult<()>`.

Math APIs return `Result<_, ContractError>`:
`validate_terms(&AuctionTerms, creation_height: u64) -> Result<(), ContractError>`;
`price_at(&AuctionTerms, tick: u16) -> Result<Uint128, ContractError>`;
`base_allocation(quantity: Uint128, tick: u16, &Clearing) -> Result<Uint128, ContractError>`;
`entitlement(allocation: Uint128, collateral: Uint128, price: Uint128,
lot_size: Uint128) -> Result<BidEntitlement, ContractError>`.
Unrevealed bids bypass `base_allocation` with zero allocation.

Commitment APIs use `CommitmentInput<'a>` with borrowed chain ID and canonical
contract/bidder/beneficiary `Addr`s, auction ID, collateral, quantity, tick and salt
slice. `commitment_bytes(&CommitmentInput) -> Result<Vec<u8>, ContractError>` and
`commitment_digest(&CommitmentInput) -> Result<[u8; 32], ContractError>` implement
the spec's exact field order/widths. The Node client exports
`encodeCommitment(input) -> Buffer`, `commitmentDigest(input) -> string` (lowercase
hex), and `newSalt() -> Buffer` (32 OS-random bytes). Its stdin JSON has
`chain_id`, `contract`, `bidder`, `beneficiary`, decimal-string `auction_id`,
`collateral`, `quantity_lots`, integer `price_tick`, and optional 64-character
`salt_hex`. If omitted, generate a salt. Output JSON includes `digest_hex`,
`digest_base64`, `salt_hex` and `salt_base64` for CosmWasm Binary inputs. It must
parse amount/ID strings into BigInt, never Number; validate addresses consistently
with the published vectors and rely on the contract for authoritative admission.

- [ ] Add failing tests for all numeric limits, underfull/exact supply pricing,
  marginal allocation, checked narrowing, canonical commitment vectors and every
  domain field changing the digest. Pin `tick_count=256`, `max_bids=256`, `B=32`
  as valid boundaries and 257/257/33 as invalid; zero also rejects.
- [ ] Run `cargo test -p native-batch-auction --test math --test commitment`
  and `node --test scripts/auction-commitment.test.mjs` before implementation;
  distinguish missing behavior from dependency/environment failures.
- [ ] Implement the frozen types, storage interfaces, arithmetic and independent
  Rust/Node encoders. Use spec-defined UTF-8 lengths and big-endian widths; add
  fixed expected byte/digest vectors, not just encoder roundtrip assertions.
- [ ] Add an independent test-only one-shot clearing oracle that sorts a complete
  book rather than sharing the incremental algorithm. The standard book is
  S=5, L=10, reserve=2, step=1, K=3; bids `(qty,tick,collateral)` are
  `(2,2,8),(2,1,6),(2,1,6),(1,0,2)` plus an unrevealed deposit of 7.

```rust
assert_eq!(clearing_price, Uint128::new(3));
assert_eq!(allocations, vec![2, 2, 1, 0, 0]);
assert_eq!(refunds, vec![2, 0, 3, 2, 7]);
assert_eq!((seller_quote, seller_sale), (15, 0));
assert_eq!(total_collateral, 29);
```

- [ ] Run the package tests, Node vectors and formatting/clippy; require all pass.
- [ ] Commit as `feat: define checked batch auction protocol and commitment encoding`.

### Task 4: Implement admission, funding controls and principal claims

**Files:** Create `src/{contract.rs,admission.rs,claims.rs,queries.rs}`,
`tests/{admission.rs,claims.rs}` and `tests/support/*`; export public entry points
from `lib.rs`. No workflow polling is required by this task's admission tests.

**Interfaces:** Match native-dca's custom entrypoint types:
`instantiate(DepsMut<RuntimeQuery>, Env, MessageInfo, InstantiateMsg)` and
`execute(DepsMut<RuntimeQuery>, Env, MessageInfo, ExecuteMsg)` return
`Result<Response<RuntimeMsg>, ContractError>`;
`reply(DepsMut<RuntimeQuery>, Env, Reply)` has the same result;
`query(Deps<RuntimeQuery>, Env, QueryMsg) -> StdResult<Binary>`.
Use register reply ID `1` and payload `RegisterReplyPayload { auction_id: u64 }`.
Register kind `native_batch_auction`, version 1 and Task 3's `WorkflowInput`.

`AuctionView { config: Config, auction: AuctionRecord, phase: BiddingPhase,
host: HostTaskView }` uses
`BiddingPhase::{Commit, Reveal, Settlement, AwaitingAbort, Finalized, Aborted}`
and `HostTaskView::{Present(TaskResponse), Unavailable { error: String,
lease_expired: bool }}`. AwaitingAbort means active business state at/after E.
Bid lookup returns `Option<BidRecord>`; pagination returns `Vec<BidRecord>` of
at most 32 records in commit order with default 16, rejecting 0 or limits over 32.
`Task` returns the authoritative native `TaskResponse` or propagates its error.

Build `AuctionSuite` over the genuine DCA-style transactional `RuntimeModule`
and `RouterPollHost`, updated for Task 2's API. Reuse their source pattern locally;
do not make auction tests include DCA business fixtures. Helpers:
`new() -> Self`, `with_fee_denom(&str) -> Self`,
`block(u64) -> AnyResult<BlockReport>`, `reconstruct() -> AnyResult<()>`,
`auction() -> AuctionView`, `bid(&Addr) -> Option<BidRecord>`.
`RejectingBank` delegates to BankKeeper except for a storage-configured recipient
and denom failure, allowing a second claim transfer to fail transactionally.

- [ ] Add tests for create-once, exact funds including fee/sale coalescing,
  registration reply mismatch/rollback, commit/reveal boundary heights, duplicate
  identities, invalid reveals leaving buckets untouched, and principal conservation.
  Use creation=1/C=10/R=20/E=60 for the standard five-bid/B=1 fixture.
- [ ] Run `cargo test -p native-batch-auction --test admission --test claims
  --all-features`; retain failing admission and claim assertions.
- [ ] Implement seller/config binding, host funding-config validation, exact
  attached funds, checked counts/demand, commitment verification before writes,
  and task reply validation. Keep instantiate nonpayable and no migrate export.
  Validate raw control task IDs against the one saved ID.
- [ ] Implement funding/control routes preserving actors. Cancel only when empty,
  atomically setting Aborted with host cancellation. Reject cancellation after
  any commitment, non-seller resume/budget changes, cap reductions after first
  commit, recipient changes and extra attached funds. Top-up does not resume.
- [ ] Implement O(1) expiry abort/lazy abort, fixed-recipient claims, claim flags
  and claimed totals. Ignore provisional entitlements in Aborted state; keep
  Finalized claims independent of host lifetime. Test finalized inputs through
  explicitly labeled fixture storage until Task 5 supplies the real finalizer.

```rust
assert_eq!(failed_second_transfer.balances_after, balances_before);
assert!(!failed_second_transfer.bid.claimed);
assert!(claim_before_finalization.is_err());
assert!(cancel_after_first_commit.is_err());
assert_eq!(abort_refund, original_collateral);
assert!(sponsor_resume.is_err());
```

- [ ] Add donation, fee=quote and fee=sale, unknown/pruned host query, empty early
  cancellation race, permissionless immutable-recipient claim, duplicate claim,
  unrevealed refund, noncanonical beneficiary, and two-instance isolation cases.
  Keep all mutating/public execute routes nonpayable except Create/Commit/Fund.
- [ ] Implement the bounded query routes. Add
  `pagination_is_exclusive_bounded_and_complete`: default returns 16, limit 32
  returns at most 32, limits 0/33 reject, and repeated exclusive cursors visit all
  256 records once without duplicates. Query tests cover every BiddingPhase and
  preserve Finalized/Aborted business state when the host query is unavailable.
- [ ] Run package tests and workspace regression/format checks; require all pass.
- [ ] Commit as `feat: add auction custody admission and guarded claims`.

### Task 5: Implement bounded settlement and generated async execution

**Files:** Create `src/{settlement.rs,workflow.rs}`, `tests/{workflow.rs,lifecycle.rs}`;
extend `contract.rs`/`lib.rs` with sudo. Add instrumented storage/fault fixtures to
test support, not production public execute messages.

**Interfaces:** `advance_batch(storage: &mut dyn Storage, height: u64,
task_id: u64) -> Result<BatchProgress, ContractError>` in `settlement.rs`, where
`BatchProgress::{Continue, Finalized, LeaseWindowExhausted}`. It performs exactly
one phase step, binds task ID, and checks business state and strict E.
Workflow helper `step(ctx: &mut WorkflowCtx, auction_id: u64)
-> Result<StepProgress, AuctionWorkflowError>` handles waiting until R and one
batch; `StepProgress { status: StepStatus, sleep_blocks: u64 }`,
`StepStatus::{Continue, Finalized, LeaseWindowExhausted}`.

`#[durable_workflow(kind = "native_batch_auction", version = 1)]`
`pub async fn native_batch_auction(ctx: WorkflowCtx, auction_id: u64)
-> Result<AuctionOutcome, AuctionWorkflowError>`, with terminal outcomes
`Finalized` and `LeaseWindowExhausted`. Define serializable
`AuctionWorkflowError::{Invariant, Storage, UnexpectedWait}`; map all WaitError
variants to UnexpectedWait. The loop checkpoints typed auction ID and StepProgress
and awaits only positive `SleepBlocks`.

`sudo(DepsMut, Env, TaskSudoMsg) -> Result<Response, ContractError>` invokes the
generated bridge, decodes its `PollResponse`, and converts **any Fail outcome**
into an outer contract error. Every workflow error in this contract is an abort
condition, not a legitimate committing application failure. This wrapper is
essential: a helper's `?` otherwise lowers to `Transition::Failed` and can commit
partial writes. Do not change the macro to implement this contract-specific policy.

- [ ] Add `incremental_matches_oracle_for_every_batch_size`,
  `one_phase_and_at_most_b_items_per_poll`, `claims_locked_during_every_phase`,
  and `failed_batch_aborts_instead_of_committing_fail` before implementation.
  Compare the standard fixture against Task 3's independently calculated results.

```rust
assert!(visited_items <= usize::from(batch_size));
assert_eq!(batch_after_error, batch_before);
assert_eq!(contract_balances_after_error, balances_before);
assert!(gas_fees_after_error >= gas_fees_before);
assert_eq!(final_allocations, vec![2, 2, 1, 0, 0]);
assert_eq!(same_height_second_run.polls, 0);
```

- [ ] Run `cargo test -p native-batch-auction --test workflow --test lifecycle
  --all-features`; observe missing settlement and atomicity failures.
- [ ] Implement ScanPrice, AllocateBase, BuildClaims and O(1) Finalize as specified.
  Use a remaining-tick cursor to avoid unsigned underflow. The residual pass
  updates at most one lot per eligible marginal bid and constructs final claims;
  do not rescan prior records or recompute the whole book on resume.
- [ ] Implement the generated loop, positive sleeps, strict deadline handling,
  task binding, sudo error conversion and no outbound payout messages. Measure
  maximum serialized workflow/checkpoint envelopes; admission reserves host metadata
  and bounded timer/result overhead, not the entire bid book.
- [ ] Add interrupted-reconstruction tests at every phase and finalization boundary;
  OOG/trap/ordinary helper error after a nonempty prefix; unknown/inconsistent
  state; sold=0; exact supply; maximum-width values; paused gas; funding without
  resume; E-1 finalization/E abort; and claims after host pruning. Abort remains
  O(1) even with all 256 bids. Business records never disappear with host pruning.
- [ ] Run the full package and workspace, strict clippy/format; require all pass.
- [ ] Commit as `feat: execute bounded auction settlement on the native scheduler`.

### Task 6: Establish compiled-Wasm, gas and transaction-boundary evidence

**Files:** Create `contracts/native-batch-auction/build-wasmd.sh`,
`examples/auction_fault.rs`, `integration/wasmd/auction_acceptance_test.go`;
update the auction manifest for the test-only artifact.

**Interfaces:** Builder sources `scripts/wasmd-normalize.sh`, respects
`CARGO_TARGET_DIR`/`WASM_OPT`, emits `artifacts/native_batch_auction.wasm` and
`artifacts/auction_fault.wasm` plus hashes. Never deploy the fault artifact in
the tutorial. `auctionAcceptanceHarness` wraps `openAcceptanceApp(t, home, ...)`
with seller/bidders/sponsor and sale/quote/fee balances. Provide
`newAuctionAcceptanceApp(t *testing.T) *auctionAcceptanceHarness`,
`(h *auctionAcceptanceHarness) block(t *testing.T, txs ...[]byte)
*abci.ResponseFinalizeBlock`, and typed auction/bid/funding/balance queries.

- [ ] Write `TestAuctionAcceptanceClearing`, `TestAuctionAcceptanceRollback`,
  `TestAuctionAcceptanceClaimsAtomic`, `TestAuctionAcceptanceFunding` and
  `TestAuctionAcceptanceDiskReopen`. Assert at least three nonempty batches,
  oracle allocations, real bank effects, no early claims, actual consumed-gas fees,
  and identical committed state after reopen.
- [ ] Add the builder and test-only partial-write fault wrapper. The latter calls
  real auction sudo then deliberately errors/traps for a selected test mode,
  so rollback is checked after real writes. Keep all fault controls out of the
  normal artifact and outside production workflow semantics.
- [ ] Build all artifacts and bootstrap the pinned environment:

```sh
bash contracts/native-fulfill/build-wasmd.sh
bash contracts/native-dca/build-wasmd.sh
bash contracts/native-batch-auction/build-wasmd.sh
export NATIVE_ASYNC_GOPATH="$(mktemp -d /private/tmp/auction-go.XXXXXX)"
export NATIVE_ASYNC_GOCACHE="$NATIVE_ASYNC_GOPATH/build"
NATIVE_ASYNC_KEEP=1 bash integration/wasmd/test.sh
```

  Retain the printed actual `GOWORK`; export it explicitly with
  `GOPATH=$NATIVE_ASYNC_GOPATH` and `GOCACHE=$NATIVE_ASYNC_GOCACHE` for targeted runs.
  Do not invent an old temporary workspace path. These caches consume several GB.
- [ ] In `integration/wasmd`, run
  `GOTOOLCHAIN=go1.23.6 go test -tags upstream -count=1 -run '^TestAuctionAcceptance' .`.
  Set up baseline failing cases before filling in scenario bodies. A missing
  artifact, skip or toolchain failure is not a passed behavior check.
- [ ] Derive deposits from the actual host's rent/allocation/attempt requirements;
  record measured per-phase caps. Test price ceiling recovery with a ceiling
  increase, not top-up alone. Check zero-price flight exclusion separately using
  the native funding callback fixtures and measured host behavior.
- [ ] Run the full pinned Go suite plus artifact hash checks; require all pass.
- [ ] Commit as `test: validate batch auction in the pinned Wasm host`.

### Task 7: Prove process-restart and network-state-sync recovery

**Files:** Create `integration/wasmd/{auction_restart_test.go,auction_state_sync_test.go}`;
reuse or narrowly parameterize existing DCA child ownership/gates and
`state_sync_network_test.go` node infrastructure, preserving old tests.

**Interfaces:** Opt-in `TestAuctionProcessRestart` and `TestAuctionStateSync`,
enabled by `NATIVE_AUCTION_NETWORK_TEST=1`. Give children a distinct
`NATIVE_AUCTION_NETWORK_CHILD` mode; do not rely on the existing hard-coded
`-test.run=^TestStateSyncNetwork$` accidentally invoking auction setup. Use
persisted commit gates to capture exact partial/finalized heights.

- [ ] Write failing scenarios for restart after a nonempty AllocateBase or
  BuildClaims batch, and after Finalized before any claim. A new child process
  must open the same home, verify PID-bound pre-handshake evidence, then resume.
- [ ] Implement the owned-child/gated restart pattern from the DCA demo. Compare
  task/funding/checkpoint, auction/cursor/bid records and bank balances before
  allowing the next block. Database reopen within the same process is not this test.
- [ ] Write/implement a three-process snapshot network: validator, independent
  snapshot-serving peer and empty joiner. Select partial-settlement and
  finalized-unclaimed snapshots; verify trusted RPC height/hash, restored marker,
  earliest block evidence and matching application state. Do not bypass a failed
  restore by deleting activation markers or reusing untrusted node data.
- [ ] Continue the restored network and uninterrupted control to matching finalized
  allocations and claimed balances. Include host-pruned claim persistence and
  rejected invalid wait state. Go's context-only guard and zero committed
  reservations remain distinct from Task 1's Rust marker/export assertion.
  Query failure never grants a refund.

```go
require.NotEqual(t, oldPID, newPID)
require.Equal(t, before.Auction, restored.Auction)
require.Equal(t, before.Bids, restored.Bids)
require.True(t, restoredBySnapshot)
require.Equal(t, control.Claims, joined.Claims)
require.Equal(t, control.Balances, joined.Balances)
```

- [ ] Run in the prepared workspace:
  `GOTOOLCHAIN=go1.23.6 NATIVE_AUCTION_NETWORK_TEST=1 go test -tags upstream
  -count=1 -timeout 10m -run '^TestAuction(ProcessRestart|StateSync)$' .`, then
  rerun existing `TestStateSyncNetwork` with `NATIVE_ASYNC_NETWORK_TEST=1`.
  Use per-RPC and overall deadlines; clean up only owned processes.
- [ ] Record exact source/artifact/snapshot identities and outcomes.
- [ ] Commit as `test: verify auction restart and state sync with retained claims`.

### Task 8: Integrate and extend the Quint models

**Files:** Import only `formal/quint/` from PR 6 head
`0619cc691e2df77a9ee8ca117415aec10544a4b0`, using scoped file patches, not a merge
of sibling runtime ancestry. Preserve its nine models, runner fixtures, lockfile
and historical receipt. Modify native model/tests/profiles and runner lists;
create `native_batch_auction{,_tests,_profiles}.qnt` and
`auction-verification-results.md`.

**Interfaces:** Preserve Quint 0.33.0 and the runner's strict exit/diagnostic checks.
Auction module exports `init`, `step`, `safety` plus witnesses
`sawSettlement`, `sawAbort`, `sawRollback`, `sawRestart`. Native model gains an
explicit guest transaction snapshot/cache so successful nested funding/delivery
and independently authorized other-task control are representable. The imported
model currently excludes these; do not claim its old flight flag proves N1 fixed.

- [ ] Record import provenance, run its unchanged portable baseline, and retain
  historical results as historical. From `formal/quint`: `npm ci`,
  `npm run parse`, `npm run typecheck`, `npm test`, `npm run verify`.
  Record Node/npm versions; the README's observed versions are not enforced pins.
- [ ] Add native deterministic tests for zero-price same-task management rejection
  across outcomes, allowed unrelated-task management, nested funding commit and
  rollback, total-liability overflow rejection, post-attempt control, strict due
  boundaries and restore without flight markers. Make assertions fail before
  extending the abstraction. Addresses are modeled classes, not a Bech32 proof.
- [ ] Implement native guest-cache commit/abort with consumed fees outside rollback.
  Keep both fixed and auction scheduling profiles and their existing checks.
  Source-map each new transition to Tasks 1/2 tests and document abstraction limits.
- [ ] Add auction deterministic tests for reserve pricing (D<S and D=S), marginal
  dust, unrevealed refunds, frozen books, item bounds, locked/repeated claims,
  two-coin claim failure, partial-write rollback, finalize E-1/reject E, abort in
  every phase, finalized-never-aborts, host-pruned claims and restart successors.
- [ ] Use three bidders/ticks, supply and quantities 1..3, collateral 1..12, B=1..2,
  C=2/R=4/E=20 and heights 0..21 for the first finite profile; filter invalid
  bids normally. E=20 satisfies the spec's worst-case margin at these bounds.
  Keep the larger standard-book dust case as a separate deterministic fixture.
  Model immutable recipients, principal separate from gas, claim flags, provisional
  allocations, cursors and full rollback snapshots.
- [ ] Add five mutation controls: provisional claims, repeated claims,
  finalize-at-E, incomplete batch rollback and lost claim flags on restore.
  Each must fail with exit 1, the exact assertion/invariant diagnostic and retained
  ITF evidence; syntax/backend failures must fail the runner rather than count.
- [ ] Extend runner registries/counts and run auction profiles with TypeScript,
  seed `20261015`, 1,000 samples, depth 160 and the four witnesses. Run
  `QUINT_RESULTS_DIR="$(mktemp -d /private/tmp/auction-quint.XXXXXX)" npm run verify`
  plus all runner tests.
  Record actual witness counts; a required witness with zero observations is a
  coverage gap, not evidence that the scenario ran. Preserve deterministic
  witness-producing cases independently of random exploration.
  Optional Apalache 0.62.1 requires an explicitly available server and remains a
  separately described bounded check, never a required all-auction theorem.
- [ ] Write fresh evidence including model/source correspondence, bounds, seeds,
  controls and exclusions.
- [ ] Commit as `formal: model auction settlement and nested async effects`.

### Task 9: Ship the runnable suite slice and verify the whole change

**Files:** Create the listed auction demo files/scripts/README and
`docs/reviews/2026-10-15-native-batch-auction-verification.md`; narrowly update
root/integration README and plan/spec status after actual completion.

**Interfaces:** `scripts/native-auction-demo.sh happy|funding|restart|expiry|all`
runs `TestNativeAuctionDemo` with `NATIVE_AUCTION_DEMO=1` and
`NATIVE_AUCTION_SCENARIO`. Use the real normal auction artifact and Node commitment
helper. Reuse DCA journal behavior: persist signed bytes/hash before broadcast,
attempted state before RPC, and resolve uncertain responses by original-hash lookup
without resubmission. Do not persist non-test signing keys.

- [ ] Add `TestAuctionDemoJournalNoRebroadcast`, `TestAuctionDemoRPCDeadline`,
  `TestAuctionDemoCleanupOwnedChild`, and shell relative-output-path tests before
  implementation. Assert one broadcast after a lost response, the original hash
  on lookup, unresolved outcomes halt the scenario, and unrelated processes survive.
- [ ] Implement loopback-only test node setup, exact funding, commit/reveal helper,
  immutable snapshot of generated salts for the current test run, bounded RPC
  deadlines, four scenarios and readable query receipts. Explain that the auction
  is capped, has no non-reveal penalty and provides no guaranteed service/fairness.
- [ ] Verify default oversubscribed clearing, pause/top-up/budget/resume behavior,
  exact-window process restart and fixed-expiry refunds. Show application finality
  separately from host status. Claims must work after host pruning.
- [ ] Run shell syntax checks and `bash scripts/native-auction-demo-test.sh`, then
  each scenario independently and `bash scripts/native-auction-demo.sh all`.
  Each run must identify source/artifact/config and exit all owned children.
- [ ] Run fresh whole-branch Rust tests, format, strict clippy, all three artifact
  builders, full pinned Go tests, both old/new gated network tests, Quint portable
  verification/runner tests, Node commitment vectors, and `git diff --check`.
  An opt-in test that skipped is not verified. Do not replace a blocked real test
  with a mock claim; preserve exact remaining gaps.
- [ ] Conduct whole-change adversarial review over custody/rounding, authority,
  host accounting/rollback, retained-state restore and source/model correspondence.
- [ ] Add regressions for scoped findings and verify fixes independently.
- [ ] Commit documentation and receipt as `docs: ship native batch auction recovery examples`.

## Acceptance, scope coverage and handoff

Spec runtime N1/funding maps to Task 1; N2/restore to Task 2; exact economics and
encoding to Task 3; admission/control/custody/queries to Task 4; bounded execution
and finalization to Task 5; actual VM/gas/transaction effects to Task 6; restart
and state sync to Task 7; abstract invariants/negative controls to Task 8; runnable
examples and final independent evidence to Task 9. This is Stage A only.

Report code implemented, tests executed and unresolved obligations separately.
Do not claim fixed findings solely because the model passes. Preserve source
and artifact hashes for every real-host/network claim. Later jobs/waitsets/IBC
remain on the approved roadmap and need their own scoped plans, not silent scope
expansion here.

The written specs are approved. This plan is awaiting review and execution-method
confirmation; no implementation task is complete. No push, new PR, merge or public
deployment is included without a subsequent request.
