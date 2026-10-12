# Async Gas Price Auction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the fixed async gas price with an EIP-1559-style base price over a fixed per-block poll budget, select polls highest bid first, and pause tasks priced out past a grace period, identically in the Rust runtime and the wasmd adapter.

**Architecture:** A pure price-update function per host, pinned by one shared JSON fixture. Each host stores `base_price` and `poll_gas_used`, charges every poll in a block at the block's base price, and updates the price after its scheduler runs. The per-contract round-robin ready queue becomes a global due index plus a global bid index, and a new bounded maintenance step pauses tasks priced out for longer than the grace period.

**Tech Stack:** Rust (cosmwasm-std `Decimal`, `Uint256`), Go 1.23.6 (Cosmos SDK v0.50.12 `LegacyDec`, `math/big`), wasmd v0.55.1.

**Spec:** `docs/superpowers/specs/2026-10-11-async-gas-price-auction-design.md`

## Global Constraints

- No new dependencies in either host.
- Rust `rustc 1.99.0 (b940084d7 2026-09-28)`; Go `GOTOOLCHAIN=go1.23.6`.
- Every price, order and pause decision must be identical in Rust and Go.
- Price arithmetic uses integer atomics (18 decimals): `mag = base_atomics * |used - target| / target / denominator`, integer division (truncation) after each step in that order; `next = base + mag` if `used > target`, else `max(floor, base - mag)`; `next` saturates at `u128::MAX` atomics.
- Target: Go `(AsyncGas - MaintenanceGas) / 2`; Rust `RuntimeConfig.block_gas / 2`. Denominator default 8.
- Eligibility is `bid >= base_price`. All polls in a block pay the block-start `base_price`.
- Bid key component: `u128::MAX - bid_atomics`, as 32 lowercase zero-padded hex digits.
- Fees go to the fee collector (unchanged).
- No step may scan all tasks; every loop is bounded by an existing or new per-block cap.
- Run `cargo fmt --all` and `gofmt` on changed files; `cargo clippy --workspace --all-features --all-targets` must report nothing.

**Running Go tests:** build the patched VM once with
`NATIVE_ASYNC_KEEP=1 bash integration/wasmd/test.sh` (prints `GOWORK=...`), then from
`integration/wasmd`: `GOTOOLCHAIN=go1.23.6 GOWORK=<printed> go test -tags upstream -count=1 -run '<Name>' .`
Rebuild artifacts with `bash contracts/native-fulfill/build-wasmd.sh` before the full run.

## Review Focus

1. **Base price above every bid.** All tasks priced out: selection polls nothing, does at most one index read, and the price decays toward the floor block by block. (Task 3, Task 6)
2. **Bid exactly equal to the base price** is eligible and polled. (Task 3, Task 6)
3. **Sustained full blocks** cannot overflow: `next_base_price` saturates at `u128::MAX` atomics instead of erroring or panicking. (Task 1)
4. **Price-paused task when the price later drops** stays paused until `resume_task`; a delivery to it is kept in the inbox. (Task 4, Task 7)
5. **`update_budget` lowering a runnable task's bid below the base price** re-keys it in the bid index and starts its grace from `max(ready height, last poll)`, not from the update. (Task 4, Task 7)

---

### Task 1: Price-update function and shared fixture

**Files:**
- Create: `fixtures/native-async/v1/base-price.json`
- Create: `crates/native-async/src/pricing.rs`; Modify: `crates/native-async/src/lib.rs` (add `mod pricing; pub use pricing::*;`)
- Create: `crates/native-async/tests/pricing.rs`
- Create: `integration/wasmd/pricing.go`, `integration/wasmd/pricing_test.go`
- Modify: `integration/wasmd/keeper_test.go` (`TestGoldenABI`: skip `base-price.json`, it is not an ABI round-trip fixture)

**Interfaces:**
- Produces (Rust): `pub fn next_base_price(base: Decimal, used: u64, target: u64, floor: Decimal, denominator: u64) -> StdResult<Decimal>` (errors only on `target == 0 || denominator == 0`); `pub fn bid_component(bid: Decimal) -> String`.
- Produces (Go): `func nextBasePrice(base sdkmath.LegacyDec, used, target uint64, floor sdkmath.LegacyDec, denominator uint64) sdkmath.LegacyDec`; `func bidComponent(bid sdkmath.LegacyDec) string`.

- [ ] **Step 1: Write the fixture.** An array of `{"name","base","used","target","floor","denominator","next"}` objects (decimals as strings) with exactly these rows:

| name | base | used | target | floor | denom | next |
|---|---|---|---|---|---|---|
| full | 1 | 2000 | 1000 | 0.001 | 8 | 1.125 |
| empty | 1 | 0 | 1000 | 0.001 | 8 | 0.875 |
| at_target | 1 | 1000 | 1000 | 0.001 | 8 | 1 |
| floor_clamp | 0.001 | 0 | 1000 | 0.001 | 8 | 0.001 |
| sub_atomic_step | 0.000000000000000003 | 2000 | 1000 | 0.000000000000000001 | 8 | 0.000000000000000003 |
| truncation | 1 | 1001 | 3000 | 0.001 | 8 | 0.916708333333333334 |
| saturate | 340282366920938463463.374607431768211455 | 2000 | 1000 | 0.001 | 8 | 340282366920938463463.374607431768211455 |

Add a `"bid_components"` array: `{"bid":"0","key":"ffffffffffffffffffffffffffffffff"}`, `{"bid":"1","key":"fffffffffffffffff21f494c589bffff"}`, `{"bid":"340282366920938463463.374607431768211455","key":"00000000000000000000000000000000"}`.

- [ ] **Step 2: Write failing tests.** Rust `base_price_fixture_matches` and Go `TestBasePriceFixture` load the fixture and assert every `next` and every bid `key` exactly. Rust also asserts `next_base_price(_, _, 0, _, 8)` and `(_, _, 1000, _, 0)` are errors.

- [ ] **Step 3: Run to verify failure.** `cargo test -p native-async --test pricing` fails to compile (no `next_base_price`); Go `-run TestBasePriceFixture` fails (undefined `nextBasePrice`).

- [ ] **Step 4: Implement both functions** with the Global Constraints formula. Rust uses `Uint256` for the product, Go `big.Int`. Neither may use `Decimal` or `LegacyDec` division (they round differently).

- [ ] **Step 5: Run to verify pass**, including `TestGoldenABI`.

- [ ] **Step 6: Commit** `feat(native-async): base price update function with shared fixture`.

---

### Task 2: Rust base price state and charging

**Files:**
- Modify: `crates/native-async/src/runtime.rs`, `crates/native-async/src/host.rs`, `crates/native-async/src/types.rs`
- Test: `crates/native-async/tests/runtime.rs`

**Interfaces:**
- Consumes: `next_base_price` (Task 1).
- Produces: `RuntimeConfig { price_grace_blocks: u64, price_adjust_denominator: u64, max_price_pauses: u32, .. }`; storage key `b"na2/base-price"` (Decimal JSON) and `b"na2/last-poll-gas"` (u64); `Runtime::base_price(&self, s: &dyn Storage) -> StdResult<Decimal>` (stored value or `keeper.config.gas_price`, which is now the floor); `Keeper::attempt_charge_at(&self, gas: u64, price: Decimal) -> StdResult<Uint128>`; `BlockReport { base_price: Decimal, next_base_price: Decimal, poll_gas_used: u64, .. }`; `FundingConfig` gains `#[serde(default)] min_base_price: Decimal, target_gas: u64, price_grace_blocks: u64, last_block_poll_gas_used: u64`.

- [ ] **Step 1: Write failing tests** in `tests/runtime.rs`, updating `setup()` with `price_grace_blocks: 3, price_adjust_denominator: 8, max_price_pauses: 10`:
  - `base_price_rises_on_full_blocks_and_decays_to_floor`: with `block_gas: 120` (target 60) and polls consuming more than 60, `report.next_base_price > report.base_price`; after idle blocks it returns exactly to the floor (`Decimal::one()` in `setup`).
  - `polls_are_charged_at_block_base_price`: store `na2/base-price = 2`; a poll consuming 7 gas plus host gas charges `ceil((7 + host_gas) × 2)`.
  - `funding_config_reports_live_price`: `RuntimeQuery::Funding(FundingConfig {})` returns `gas_price` equal to the stored base price and `min_base_price == 1`.

- [ ] **Step 2: Run to verify failure.** `cargo test -p native-async --all-features --test runtime base_price polls_are_charged funding_config_reports`.

- [ ] **Step 3: Implement.** Read the base price once at the start of `run_block_cached`; replace every `self.keeper.config.gas_price` and `attempt_charge(..)` call in `runtime.rs` with the block's price via `attempt_charge_at`; accumulate actual poll gas; after the visit loop compute and store `next_base_price(base, used, block_gas / 2, floor, price_adjust_denominator)` and `na2/last-poll-gas`. Outside `run_block`, `register`, `manage` and `update_policy` use `base_price(s)`. The legacy `Keeper::execute` path keeps its fixed price.

- [ ] **Step 4: Run all native-async tests**, then commit `feat(native-async): moving base price in Rust runtime`.

---

### Task 3: Rust bid-ordered selection

**Files:**
- Modify: `crates/native-async/src/runtime.rs`
- Test: `crates/native-async/tests/runtime.rs`

**Interfaces:**
- Consumes: `bid_component` (Task 1), `Runtime::base_price` (Task 2).
- Produces: `ExecutionTask.ready_at: u64` (`#[serde(default)]`, set to `u64::MAX` in `check_quota`'s reserved copy); bid index key `[b"na2/bid/", bid_component(bid).as_bytes(), &ready_at.to_be_bytes(), &seq.to_be_bytes()].concat()`; `fn ready(s, t, bid: Decimal, h: u64)`. Removes `ring`, `ready_prefix`, `member_key`, `remove_empty_ring` and the `na2/ring*` keys.

- [ ] **Step 1: Write failing tests:**
  - `highest_bid_polls_first_and_equal_bids_are_fifo`: three tasks bidding 3, 5, 5 (the two 5s registered in order) with a budget for one poll per block are polled 5(first), 5(second), 3.
  - `stops_at_first_bid_below_base_price`: base 4, bids 5 and 3 → only the 5 polls; `report.visits <= 2`.
  - `bid_equal_to_base_price_is_eligible` (Review Focus 2).
  - `all_priced_out_polls_nothing_and_decays` (Review Focus 1): base above every bid, `report.polls == 0`, `report.visits <= 1`, and `next_base_price < base_price`.
  - `oversized_task_is_skipped_then_first_next_block`: a high bid whose `gas_per_poll` exceeds the remaining budget is skipped, a smaller lower bid polls, and next block the high bid polls first.
  - `update_budget_rekeys_bid_entry`: raising a waiting-in-index task's bid moves it ahead of a higher prior bid.
  - Update existing round-robin tests (`round_robin_contracts_fifo_tasks_and_deferred_head_survive_restart`, `cancellation_churn_removes_empty_ring_before_releasing_storage`) to assert bid order and no `na2/ring` keys.

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement.** `ready` keys the bid index and records `ready_at = h`; promotion passes the block height; the visit loop walks `first`-style from `na2/bid/` up to `max_visits`, stopping at the first bid below the base price and skipping (not removing) entries whose `gas_per_poll` exceeds the remaining budget. `update_policy` re-keys a task holding a bid entry, keeping `ready_at`.

- [ ] **Step 4: Run all native-async tests**, then commit `feat(native-async): bid-ordered selection in Rust runtime`.

---

### Task 4: Rust grace pause, registration floor and resume

**Files:**
- Modify: `crates/native-async/src/runtime.rs`
- Test: `crates/native-async/tests/runtime.rs`

**Interfaces:**
- Consumes: bid index (Task 3), `price_grace_blocks`, `max_price_pauses` (Task 2).
- Produces: `BlockReport.price_pauses: u32`.

- [ ] **Step 1: Write failing tests:**
  - `priced_out_task_pauses_only_after_grace`: base above the bid, grace 3, task ready at h; no pause at h+2, `TaskStatus::Paused { reason: PriceLimit, resume: Runnable }` by h+3, and its bid entry is gone.
  - `full_block_wait_never_price_pauses`: bid at or above base but budget exhausted by higher bids for 10 blocks, never paused.
  - `price_pause_survives_price_drop_until_resume` (Review Focus 4): after pausing, the price returns to the floor; still paused; a delivery is stored in the inbox; `update_budget` then `resume_task` makes it runnable.
  - `lowered_bid_grace_counts_from_last_poll` (Review Focus 5).
  - `registration_rejects_bid_below_floor_admits_below_base`.
  - `resume_requires_bid_at_or_above_base_price`.

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement** the `price` maintenance pass in `run_block_cached` after timer promotion and before selection: iterate `na2/bid/` with `Order::Descending`, at most `max_price_pauses` items each charged through `self.maintenance`, stop at the first bid `>= base`, pause when `h - max(ready_at, last_attempt.unwrap_or(0)) >= price_grace_blocks`. Registration rejects `max_gas_price < floor`; `manage` resume compares against `base_price(s)`.

- [ ] **Step 4: Run all native-async tests**, then commit `feat(native-async): price grace pause in Rust runtime`.

---

### Task 5: Go base price state and charging

**Files:**
- Modify: `integration/wasmd/keeper.go`, `config.go`, `scheduler.go`, `management.go`, `snapshot_state.go`
- Test: `integration/wasmd/pricing_test.go`, and existing tests that set `Config.GasPrice`

**Interfaces:**
- Consumes: `nextBasePrice` (Task 1).
- Produces: `Config.MinBasePrice sdkmath.LegacyDec` (renamed from `GasPrice`), `Config.PriceGraceBlocks uint64`, `Config.PriceAdjustDenominator uint64` (ExperimentalConfig: floor `0.001`, grace `10`, denominator `8`); store keys `base_price` (decimal string) and `last_poll_gas`; `func (k *Keeper) basePrice(ctx sdk.Context) sdkmath.LegacyDec`; `func (k *Keeper) feeAt(gas uint64, price sdkmath.LegacyDec) sdkmath.Int`; state manifest schema `2`.

- [ ] **Step 1: Write failing tests:** `TestBasePriceRisesAndDecays` (drive `EndBlock` with stored tasks that consume more than the target, read `base_price`, then idle blocks back to the floor); `TestPollChargedAtBlockBasePrice`; `TestFundingConfigReportsLivePrice` (query `funding_config` returns `gas_price` = stored base price plus `min_base_price`, `target_gas`, `price_grace_blocks`, `last_block_poll_gas_used`); `TestConfigRejectsZeroDenominator`.

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement.** `EndBlock` reads the base price once into the scheduler context (a context value like `schedulerKey`), `poll` charges with `feeAt`, actual execution gas accumulates, and after the visit loop the price is updated and `async_base_price` is emitted with `old`, `new`, `used`, `target`; `async_poll` gains `price`. Replace every `Config.GasPrice` read (`keeper.go`, `management.go`, `scheduler.go`) with the base price; `fee(gas)` stays as `feeAt(gas, basePrice(ctx))` where a context is available. `ValidateState` expects `base_price` and `last_poll_gas` when present and checks `base_price >= MinBasePrice`; bump the manifest schema to `2`.

- [ ] **Step 4: Run Go targeted tests and the whole keeper package**, then commit `feat(wasmd): moving base price in Go adapter`.

---

### Task 6: Go due and bid indexes

**Files:**
- Modify: `integration/wasmd/keeper.go` (`enqueue`, remove `ensureContract`, `rotateContract`, `cleanContract`), `scheduler.go` (`selectReady`, new promotion), `management.go` (`UpdateBudget` re-key), `snapshot_state.go`
- Test: `integration/wasmd/safety_test.go`, new `integration/wasmd/selection_test.go`

**Interfaces:**
- Consumes: `bidComponent` (Task 1), `basePrice` (Task 5).
- Produces: due key `due/{height:020}/{seq:020}`; bid key `bid/{bidComponent}/{readyAt:020}/{seq:020}`; `func (k *Keeper) enqueue(ctx sdk.Context, t *Task, height uint64)` writes the due index when `height > ctx.BlockHeight()` (or during the scheduler phase) and the bid index otherwise; `func (k *Keeper) promoteDue(ctx sdk.Context) bool` (one entry per call, run under `MaxMaintenance` like other kinds); `func (k *Keeper) selectReady(ctx sdk.Context, remaining uint64, skipped [][]byte) (*Task, bool)`. Removes the `contracts/`, `contract-queue/`, `contract_sequence` and `ready/` keys.

- [ ] **Step 1: Write failing tests** mirroring Task 3: `TestHighestBidFirstEqualBidsFIFO`, `TestStopsAtFirstBidBelowBase`, `TestBidEqualToBaseEligible`, `TestAllPricedOutPollsNothing`, `TestOversizedSkippedThenFirstNextBlock`, `TestUpdateBudgetRekeysBid`, `TestValidateStateRejectsContractQueueKeys`. Replace `TestPersistentContractRoundRobin` with the bid-order test and keep `TestSelectReadyKeepsSkippedTasksConsistent`, asserting a task polled this block moves to `due/` at `h+1`.

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement.** Promotion runs before selection and moves due entries with height `<= h` into the bid index (skipping terminal or paused tasks, deleting their entry). `selectReady` walks `bid/` from the front within `MaxVisits`, stopping at the first bid below base and passing over entries in `skipped`. `UpdateBudget` re-keys a task with a bid entry. `ValidateState`: for each runnable task, `QueueKey` must equal the due or bid key computed from its fields, and the removed key families must be absent.

- [ ] **Step 4: Run Go targeted tests, then the full adapter package**, then commit `feat(wasmd): bid-ordered selection with due index`.

---

### Task 7: Go grace pause, registration floor and resume

**Files:**
- Modify: `integration/wasmd/scheduler.go` (`maintenanceOne` gains kind `price`), `keeper.go` (`RegisterTask`, `Management` resume)
- Test: `integration/wasmd/selection_test.go`, `registration_policy_test.go`

**Interfaces:**
- Consumes: bid index (Task 6), `PriceGraceBlocks` (Task 5).

- [ ] **Step 1: Write failing tests** mirroring Task 4: `TestPricedOutPausesOnlyAfterGrace`, `TestFullBlockWaitNeverPricePauses`, `TestPricePauseSurvivesPriceDropUntilResume`, `TestLoweredBidGraceCountsFromLastPoll`, `TestRegistrationRejectsBidBelowFloor`, `TestResumeRequiresBidAtBase`.

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement** `func (k *Keeper) pricePauses(ctx sdk.Context)`, called once from `EndBlock` after the `timer` maintenance kind inside one `cachedMaintenance`. It walks `bid/` with `ReverseIterator` (collect up to `MaxMaintenance` keys first, then close the iterator before writing), stops at the first bid `>= base`, pauses (`Status = "paused"`, `Pause = "price_limit"`, delete bid key, `QueueKey = nil`) each entry with `h - max(ReadyAt, LastPoll) >= PriceGraceBlocks`, and skips younger entries, matching Rust's skip semantics. `RegisterTask` rejects `MaxGasPrice < MinBasePrice`; resume compares against `basePrice(ctx)`.

- [ ] **Step 4: Run Go targeted tests**, then commit `feat(wasmd): price grace pause`.

---

### Task 8: End-to-end congestion, docs and full verification

**Files:**
- Create: `integration/wasmd/price_auction_test.go` (`//go:build upstream`)
- Modify: `docs/superpowers/specs/2026-10-11-native-cosmwasm-async-design.md` ("Gas, escrow, and storage economics": point to the auction spec), `crates/native-async/README.md`, `integration/wasmd/README.md`

- [ ] **Step 1: Write the end-to-end test** `TestCongestionRaisesPriceAndPausesLowBidder` using the real `native_fulfill_example.wasm` app harness from `app_integration_test.go`: register enough high-bid tasks to exceed the target for several blocks plus one low bidder; assert the base price rises monotonically while congested, the low bidder is `paused`/`price_limit` after `PriceGraceBlocks`, and after cancelling the high bidders the price returns to `MinBasePrice`.

- [ ] **Step 2: Run it.** It should pass on top of Tasks 5-7; if it fails, fix the owning task's code, not the test.

- [ ] **Step 3: Update the docs** listed above: the moving price, bid ordering, grace pause and the new `FundingConfig` fields.

- [ ] **Step 4: Full verification.** `cargo test --workspace --all-features` (0 failed), `cargo clippy --workspace --all-features --all-targets` (no output), rebuild artifacts, `bash integration/wasmd/test.sh` (exit 0).

- [ ] **Step 5: Commit** `test(wasmd): end-to-end congestion pricing; docs`.

