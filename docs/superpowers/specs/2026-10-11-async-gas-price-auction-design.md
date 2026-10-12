# Async gas price auction design

Status: implemented in the Rust reference runtime and experimental wasmd adapter;
not production activated. Amends "Gas, escrow, and storage economics" in
[2026-10-11-native-cosmwasm-async-design.md](2026-10-11-native-cosmwasm-async-design.md),
which originally fixed a single consensus-defined async gas price.

## Goal

Price congestion in the native async poll budget. Each block has a fixed
budget for polls. When runnable demand exceeds it, the price rises until
low-value work waits, then pauses; when demand falls, the price decays to a
governance floor. Under congestion, higher bids are polled first.

Decisions taken during design:

| Question | Decision |
|---|---|
| Purpose | Congestion pricing (EIP-1559-style base price), not a sealed-bid auction |
| Budget | Fixed per block; no carry-over; polls stay in EndBlock |
| Order under congestion | Highest `max_gas_price` (bid) first; all polls pay the base price |
| Base price revenue | Fee collector, as today |
| Update rule | Target 50% of the poll budget, at most ±12.5% per block, floor at today's price |
| Priced-out tasks | Wait for a grace period, then pause with `price_limit` |

Out of scope: execution off the block's critical path (vote extensions or
proposer execution), carry-over budgets, priority tips, and migrating existing
state (nothing is deployed).

## Invariants

- Every price, order and pause decision is consensus-deterministic and
  identical in the Rust reference runtime and the wasmd adapter.
- Per-block work stays bounded by existing caps (`MaxVisits`,
  `MaxMaintenance`, promotion caps). No step scans all tasks.
- A poll never pays more than its task's bid, and every poll in a block pays
  the same base price.
- Expiry still takes precedence: no poll at or after a task's lease end.

## 1. Pricing state and update rule

New consensus state, in both hosts:

- `base_price`: decimal price per gas, initialised to the floor.
- `poll_gas_used`: gas actually consumed by polls in the current block.
  Maintenance and host accounting are excluded. Reset every block.

Configuration:

| Today | New | Meaning |
|---|---|---|
| Go `Config.GasPrice`, Rust `FundingConfig.gas_price` | `MinBasePrice` / `min_base_price` | Floor for `base_price` |
| (none) | `PriceGraceBlocks` / `price_grace_blocks` | Blocks a priced-out task may wait before pausing |
| (none) | `PriceAdjustDenominator` / `price_adjust_denominator` | 8, giving at most ±12.5% per block |

The target is derived, not configured, so it cannot disagree with the budget:
Go `target = (AsyncGas − MaintenanceGas) / 2`; Rust
`target = RuntimeConfig.block_gas / 2`. Config validation rejects a zero
target, zero denominator or a negative floor.

Per block:

1. EndBlock reads `base_price` once. Every reservation, charge and
   eligibility check in the block uses it, replacing every read of the fixed
   config price.
2. Each poll adds its consumed gas to `poll_gas_used`.
3. After the scheduler finishes:

   ```text
   delta = base × (used − target) / target / denominator
   next  = max(floor, base + delta)
   ```

   Arithmetic is 18-decimal fixed point (Rust `Decimal`, Go `LegacyDec`).
   Compute the magnitude `|delta| = base × |used − target| / target /
   denominator`, truncating after each multiplication and division in that
   order, then add it (`used > target`) or subtract it, saturating at the
   floor. Truncating the magnitude makes both increases and decreases slightly
   smaller, never larger. A shared fixture pins the result.
4. The update runs even when nothing polled, so an idle chain decays to the
   floor.

Fees continue to flow to the fee collector.

## 2. Selection

The per-contract round-robin ready queue (Go `contracts/`, `contract-queue/`
and `ready/{owner}/` keys; Rust ring and per-contract ready prefixes) is
replaced by two global indexes:

- **Due index**, `due/{height:020}/{seq:020}`: tasks that become runnable at a
  future height (registration, yield, timer, delivery during EndBlock, a task
  already polled this block). A bounded promotion step moves due entries into
  the bid index at the start of the scheduler. Rust already has promotion; Go
  adopts it.
- **Bid index**, `bid/{inverted bid}/{ready height:020}/{seq:020}`: tasks
  runnable now. `inverted bid = u128::MAX − bid_atomics`, where `bid_atomics`
  is the bid as an 18-decimal fixed-point integer, encoded as 32 zero-padded
  hex digits so lexical order equals numeric order. Highest bid sorts first;
  equal bids are first in, first out.

Selection each block:

1. Walk the bid index from the front, at most `MaxVisits` entries.
2. Stop at the first bid below `base_price`; nothing after it is eligible.
3. Skip a task whose `gas_per_poll` exceeds the remaining poll budget and try
   the next. Skipped tasks stay at the front, so the next block (with a full
   budget) reaches them first. Every valid `gas_per_poll` fits an empty
   budget. This avoids starvation caused solely by fitting smaller tasks into a
   block remainder; sustained higher-bid arrivals can still starve lower bids.
4. A task already polled this block moves to the due index at `h + 1`.
5. The existing `insufficient_funds` check uses `gas_per_poll × base_price`.

`update_budget` re-keys a task's bid-index entry when its bid changes.
Fairness across contracts now comes from price plus the per-contract admission
cap (`MaxContractTasks` / `max_contract_tasks`), not from taking turns.

## 3. Pausing, resuming and registration

**Grace pause.** A maintenance step named `price` runs with expiry, prune and
timer maintenance, under the same `MaxMaintenance` cap. It walks the bid
index from the back (lowest bids first; Go `ReverseIterator`, Rust `range`
with `Order::Descending`):

- Stop at the first bid at or above `base_price`.
- If `h − max(ready height, last poll) ≥ PriceGraceBlocks`, pause the task
  with `price_limit`, remove its bid entry, and keep its continuation and
  inbox.
- Otherwise skip it.

Under load the pause can come later than the grace period, never earlier. A
task waiting only because blocks are full (bid at or above the base price)
never pauses for price.

**Charges.** The poll reservation is `gas_per_poll × base_price`, never more
than `gas_per_poll × bid`. `max_attempt_charge` in queries uses `base_price`.

**Registration.** Rejected if the bid is below `MinBasePrice`. A bid below
the current `base_price` is admitted and waits (and may grace-pause). Required
funding is the lease plus one poll at the current `base_price`.

**Resume.** `resume_task` requires `bid ≥ base_price` and escrow for one poll
at `base_price`. The usual path out of a price pause is `update_budget` to
raise the bid, then `resume_task`.

**Unchanged.** A delivery to a price-paused task stays in its inbox and the
task stays paused. Waiting and paused tasks still expire at their lease end.

## 4. Interfaces and observability

- `FundingConfig` keeps `gas_price`, which now reports the current base price,
  and adds `min_base_price`, `target_gas`, `price_grace_blocks` and
  `last_block_poll_gas_used`, all `#[serde(default)]` / `omitempty`.
- `TaskFunding.max_attempt_charge` uses the current base price.
- Go emits `async_base_price` each block (old price, new price, gas used,
  target). `async_poll` gains `price`.
- Rust `BlockReport` gains `base_price`, `next_base_price` and
  `poll_gas_used`.
- Shared wire fixtures under `fixtures/native-async/v1/` are updated; both
  hosts' fixture tests must pass.
- `ValidateState` validates the due and bid indexes and the new price keys,
  and rejects the removed contract-queue keys. The state manifest's schema
  version is bumped, so earlier snapshots are refused.
- Docs: the "Gas, escrow, and storage economics" section of the native design
  spec points here; both READMEs describe the moving price.

## 5. Testing

Test-first, in both hosts:

- **Price fixture** (`fixtures/native-async/v1/base-price.json`): rows of
  (base, used, target, floor, denominator) → next, covering a full block, an
  empty block, at target, clamped to the floor, both ±12.5% limits, and
  rounding. Rust and Go each run every row.
- **Ordering:** highest bid first; equal bids first in, first out; stop at the
  first bid below the base price; an oversized task is skipped, then served
  first in the next block; a task polled this block moves to `h + 1`.
- **Grace pause:** pauses only after `PriceGraceBlocks` blocks without a poll;
  a congested task bidding at or above the base price never pauses;
  `update_budget` plus `resume_task` restores it.
- **Bounded work:** selection and the grace walk each touch at most their
  caps, whatever the index size.
- **State validation and snapshots:** the new indexes are accepted; stale bid
  entries, missing due entries and old contract-queue keys are rejected.
- **End to end (Go, real Wasm):** multi-block congestion raises the price; a
  low bidder is priced out and pauses after the grace period; when demand
  stops the price decays to the floor.
