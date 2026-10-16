# Native multi-block sealed-bid auction and runtime safety baseline

Date: 2026-10-15

Status: written specification approved on 2026-10-15. The
[implementation plan](../plans/2026-10-15-native-batch-auction.md) awaits review;
the rules below are not yet implemented or tested.

## Purpose and scope

Deliver Stage A of the [async showcase suite](2026-10-15-native-async-showcase-suite-design.md):
repair the native runtime safety boundaries used by the examples, then build a
sealed-bid auction whose settlement is deliberately divided across blocks.
Show checkpointed computation, bounded work per poll, independent gas funding,
restart/state-sync recovery, and safe principal recovery when settlement expires.

This is an application auction selling test assets. It is not the scheduler's
existing gas-price auction, a production market, or an economic fairness proof.
The implementation base is DCA PR 7 at
`5566d5bd3fd37a02fffc725c919f3980a1187b46`. Integrate the needed formal tooling from
PR 6 at `0619cc691e2df77a9ee8ca117415aec10544a4b0` deliberately; that sibling branch
is not currently part of the base. Refresh both heads when planning execution.

Do not add a waitset ABI, cross-contract jobs, IBC, a DEX, or a new scheduler
fairness policy in this implementation. The contract uses existing sequential
generated workflows and `SleepBlocks`.

## Part 1: runtime safety baseline

### N1: explicit in-flight exclusion

Rust currently infers an in-flight attempt from nonzero fee reservations in
[`runtime.rs`](../../../crates/native-async/src/runtime.rs). At zero gas price
that inference fails. Add an explicit execution-scoped identity for the polling
`(contract, task_id)`, independent of money. Every same-task resume, cancellation,
lease extension and budget/refund-policy update must consult it before mutation.
Preserve ordinary authority checks and isolation from unrelated tasks.

The identity must be visible to nested dispatch/management calls; a local variable
that those paths cannot inspect is insufficient. It exists through guest polling
and outbound dispatch, including terminal responses, and is removed before a
successful guest cache commits. A private marker in the nested execution cache
provides this visibility without adding a persisted task field. Error and abort
paths transactionally discard it. Fee settlement remains inside the enclosing
atomic block transition; no marker may survive a committed block or accepted
snapshot. Retain reservation-accounting checks as separate invariants, not as the
execution lock. Do not add this transient identity to the
guest wire ABI or to durable continuation bytes.

Authenticated callbacks and `FundTask` remain allowed while polling, matching the
Go host. Successful nested deposits must survive the task/funding reload before
settlement; failed poll effects, including nested deposits, roll back together.
Before accepting any deposit require `available + reserved + deposit` to fit
`Uint128`, not just `available + deposit`. Source inspection found that the Go
funding path checks that total while Rust's funding keeper currently only checks
available funds. This is a scoped accounting requirement to reproduce and test,
not a claim of an already demonstrated chain failure.

Require zero- and positive-price regressions with callbacks attempting each
management operation during Pending, Yield, Complete and Fail outcomes. Check
guest-error/OOG rollback, actual fee settlement, independent-task management,
successful in-flight funding, overflow rejection before bank movement, and
successful management after the attempt has ended. Cover the actual nested
transactional path, not only a manually set guard flag.

### N2: common pending-wait admission

Both hosts must require `poll_height < due_height < task.expires_at` for height
timers and operation deadlines before committing a new checkpoint or dispatching
messages. A response at or beyond task expiry is invalid; rejection rolls back
guest effects and charges consumed gas according to the existing failure policy.
This does not change consumption of an outcome already accepted before its
operation deadline.

Use the configured chain's canonical account-address policy for resolvers.
Validate through the trusted host API, require the validated string to equal the
input, and reject malformed, wrong-prefix or noncanonical addresses consistently.
Rust's `run_block`/`stage` currently have no `Api`; thread `&dyn Api` through the
reference-runtime execution path rather than pretending guest-side validation
protects raw `PollResponse` inputs. Update local callers and fixtures. Go uses a
shared parse-and-canonical-equality helper at admission and restoration. No change
to serialized `Wait` fields is required.

Add shared canonical-address and due-boundary vectors for both hosts, including
due equal to current height, expiry minus one, expiry, and expiry plus one;
malformed/overlong resolver; valid canonical resolver; and rejected alternate
spelling. Use real Bech32 fixtures rather than unconstrained labels in parity
tests. Byte-format compatibility does not mean old invalid states become valid:
restore must fail closed on out-of-policy waits and must not silently discard or
rewrite them. Existing valid active v1 tasks must round-trip unchanged. Because
this is a prototype, upgrades with invalid historical reference-runtime state
require an explicit operator migration decision outside this change.

### Evidence boundary

The [prior review](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/0619cc691e2df77a9ee8ca117415aec10544a4b0/docs/reviews/2026-10-12-all-open-pr-async-review.md)
documents source findings, not runtime fixes. Obtain failing implementation
regressions first, then repairs and passing differential tests. Preserve R1 as
an explicit scheduling-policy limitation; no unconditional liveness claim is
introduced by this auction.

## Part 2: auction rules

### Instance and immutable terms

Create `contracts/native-batch-auction`, with one auction per contract instance
and no migration entry point. Instantiate without funds; record the actual
instantiator as seller and configure distinct native sale/quote denominations.
Validate the fee denomination against the host funding configuration. The local
demo instantiates with no chain-level admin, so an administrator cannot replace
the code and rewrite liabilities.

`CreateAuction` is seller-only and succeeds at most once. It fixes:

- Auction ID `1`, seller and payout address (the same immutable address).
- Supply `S` in integer lots and sale base units per lot `L`, both positive.
- A strictly increasing price grid `p[t] = reserve + t * tick_size`, with
  positive integer quote base units per lot, `tick_size > 0`, and
  `1 <= tick_count <= 256`.
- `max_bids` in `1..=256`, one immutable commitment per bidder address, and
  `batch_size B` in `1..=32`.
- Commit close `C`, reveal close `R`, and settlement/task expiry `E`, satisfying
  `creation_height < C < R < E`.
- Initial gas funding, gas-per-poll cap, price ceiling and task byte allocation.

Require `R + Pmax + 1 < E`, with checked arithmetic and
`Pmax = ceil(tick_count/B) + 2*ceil(max_bids/B) + 4`. This leaves room for the
worst-case bounded passes under timely service; it is not an execution guarantee.
The host's lifetime/admission limits also apply. No phase or expiry extension is
exposed after creation.

Validate representability of `S*L`, `S*p[max_tick]`, every grid price, and aggregate
accepted collateral. Store total and per-tick demand as checked `Uint256`; use
checked `Uint256` intermediates for pro-rata products. Quantities, supply, prices,
coin liabilities and payouts fit `Uint128`. Reject overflow rather than
saturating. No floating-point arithmetic is permitted.

Creation attaches exactly `S*L` sale units plus the explicit initial gas coin;
coalesce amounts with checked addition if the fee and sale denomination match.
Reject zero, duplicate, missing, unexpected or excess coin entries. Only gas
funds enter host escrow. Registration and auction creation are atomic; a reply
binds the actual host task ID and verifies owner, workflow kind/version and E.

### Commitments and reveals

Both commit and reveal require a created, nonterminal auction. For `height < C`,
`CommitBid { digest, collateral }` accepts exactly that positive quote collateral.
It records the validated actual bidder address, monotonic
commit index, 32-byte digest, and collateral. It increments the checked aggregate
collateral. Reject a second commitment from the same bidder and admission above
`max_bids`. There is no amendment, replacement, or early collateral withdrawal.
The address cap is not a Sybil-resistance claim.

For `C <= height < R`, only that bidder may call
`RevealBid { quantity_lots, price_tick, beneficiary, salt }`, without funds.
Validate `1 <= quantity_lots <= S`, a valid tick, a canonical beneficiary, an
exactly 32-byte salt, the matching digest, and
`quantity_lots * p[price_tick] <= collateral`. Overcollateralization is allowed;
collateral is public and may reveal an upper bound. Salt entropy is the bidder's
responsibility; the client uses an OS cryptographic RNG, not a predictable seed.

Commitment digest is SHA-256 of the following exact concatenation. This is a
versioned binary format, not JSON:

1. Raw ASCII domain bytes `cosmwasm/native-batch-auction/bid/v1` (no terminator).
2. Chain ID, contract address, bidder address, and beneficiary address, in that
   order; each encoded as a 4-byte unsigned big-endian UTF-8 byte length followed
   by its UTF-8 bytes. Addresses use their validated canonical string form.
3. Auction ID as 8-byte unsigned big-endian (`1`).
4. Collateral as 16-byte unsigned big-endian.
5. Quantity in lots as 16-byte unsigned big-endian.
6. Price tick as 2-byte unsigned big-endian, zero-based.
7. The 32 salt bytes verbatim.

Reject encoding lengths outside their integer domain. Chain ID and addresses are
bound so a commitment cannot be reused by another bidder, contract or chain.
Publish byte-level hash vectors for contract and client implementations.

A successful reveal atomically records the terms and increments that tick's
demand bucket and total revealed demand. Reject duplicate reveals. Invalid
reveals leave all state unchanged and can be corrected before R. At or after R,
reveal admission is closed regardless of scheduler progress, so settlement reads
immutable inputs. Unrevealed bids receive zero allocation and full collateral
refund; the first version has no non-reveal penalty.

### Uniform-price allocation

Let D be total revealed demand. If `D <= S`, every revealed bid receives its full
requested quantity and pays the reserve price `p[0]`; unsold lots are `S-D`.
This explicitly includes `D == S`, zero bids and no valid reveals.

If `D > S`, scan price ticks in descending order to find the marginal tick t:
strictly higher demand H is below S and `H + demand[t] >= S`. Define
`M = S-H` and `Dm = demand[t]`.

- Higher-tick bids receive their full requested quantity.
- Lower-tick and unrevealed bids receive zero.
- Marginal bids initially receive `floor(quantity_i * M / Dm)` lots.
- Distribute the remaining marginal lots one each, in ascending commit index,
  to marginal bidders whose allocation is below their requested quantity, until
  no remainder remains.

Every winner pays the same `P = p[t]` quote units per lot. Commit-order dust
allocation is deterministic, not economically neutral or ordering-resistant.
Prove the remainder is smaller than the number of marginal bids and the single
residual pass can exhaust it; do not introduce an unbounded corrective loop.

## Bounded settlement and workflow

Store bidding records and per-tick demand in application storage. Store only
small workflow captures, such as auction ID; do not serialize all bids in each
checkpoint. Application settlement state holds the authoritative phase, cursor,
clearing data, provisional allocations, residual and running totals.

The generated `native_batch_auction/v1` workflow waits until R using positive
`SleepBlocks`, then calls one bounded settlement step per poll. If already at or
past R when first scheduled, it starts directly without `SleepBlocks(0)`.

Settlement phases are:

1. `ScanPrice`: inspect at most B descending tick buckets. Underfull auctions
   select the reserve without scanning. Persist the next cursor or clearing data.
2. `AllocateBase`: inspect at most B commitments in commit order and write each
   provisional base allocation. Accumulate the total base allocation. At the
   end compute residual lots relative to `min(D,S)`.
3. `BuildClaims`: inspect at most B commitments in commit order. Apply at most
   one residual marginal lot per eligible bid; calculate that bid's final sale
   entitlement and quote refund; accumulate final liabilities and seller proceeds.
   Check each allocation is at most the revealed quantity, each payment is at
   most its collateral, and unrevealed bids have zero allocation/payment.
4. `Finalize`: constant work checking all cursors exhausted, residual zero,
   allocated lots equal `min(D,S)`, and the aggregate conservation equations.
   Atomically set the business state to `Finalized` and return workflow completion.

Each invocation performs at most one phase step, with at most B item visits;
empty-phase transitions are also bounded steps. There is no nested bidder/tick
scan. Total item visits are bounded by `tick_count + 2*committed_bids`, plus
fixed transitions. Persist cursors, phase and changed records atomically with
the new checkpoint. Finalization never scans the whole bid list.

Between unfinished steps, await `SleepBlocks(1)`. If the next timer would have
`height+1 >= E`, complete with `LeaseWindowExhausted` and leave business claims
locked; recovery becomes available at E. Every settlement helper independently
requires `height < E` and a nonterminal business state. It must bind the current
host task ID from the workflow correlation to the saved auction task ID.

There is no public `AdvanceBatch` bypass. Auction writes occur through the
privileged poll path. A malformed state, arithmetic inconsistency or OOG must
abort the poll transaction and pause according to host policy, not return an
application `Fail` that commits partially invalid writes.

No bank transfers occur during computation. The final helper writes application
state before returning; terminal workflow outputs need not carry messages.
Public claim transactions perform all payouts separately.

## Principal accounting, claims, and abort

For each bidder i, let `c_i` be deposited collateral, `a_i` final allocated lots,
and P the clearing price. Finalization checks:

```text
A = sum(a_i) = min(D, S)
bidder sale_i = a_i * L
paid_i = a_i * P <= c_i
refund_i = c_i - paid_i
seller quote = A * P
seller sale = (S - A) * L
sum(refund_i) + seller quote = sum(c_i)
sum(bidder sale_i) + seller sale = S * L
```

Provisional entitlements are never claimable. `ClaimBid { bidder }` and
`ClaimSeller` are permissionless triggers with immutable recipients: sale assets
go to the revealed beneficiary, collateral refunds to the committing bidder,
and seller proceeds/unsold inventory to the seller. Callers cannot redirect
funds. Each bid has one claim flag and the seller has one claim flag. Set flags,
update claimed totals and send all nonzero payouts in one atomic transaction;
any bank failure rolls everything back. Reject repeated claims. Claims accept no
funds and perform O(1) work.

Before finalization, `Abort` is permissionless only at `height >= E`. It sets
`Aborted` irreversibly in O(1), ignores all provisional allocations, and exposes
full quote-collateral refunds for every bidder plus full sale inventory for the
seller. Claims compute those abort entitlements directly; no sweeping pass is
required. A claim may perform the same lazy abort transition when eligible.

An additional seller-only early cancellation is allowed only while the auction
has zero commitments and is not finalized/aborted. It atomically requests host
cancellation and sets `Aborted`; if host cancellation fails, all changes roll
back. Once the first commitment exists, expose no host cancellation route from
this contract. Unexpected host termination never authorizes early principal
recovery. At E recovery no longer depends on an unpruned/queryable host task.

Finalization at E-1 preserves finalized claims; finalization at E is forbidden.
An already finalized auction never becomes aborted, even after host expiry,
pruning, gas refund, or an unavailable host query. Likewise an aborted auction
cannot resume settlement. Claim rights have no expiry or administrative sweep
in this example. Application records are permanently bounded by one auction and
at most 256 bids; they are not pruned with the host task or claimed to be covered
by its storage lease. Donations do not enlarge entitlements and have no sweep
route in v1.

Gas refunds are separate from principal. The host's normal configured refund
recipient is the seller. Do not use the contract's raw bank balance to infer
sale inventory, bidder collateral, or recoverable gas.

## Funding, control and public queries

Expose `Async(FundTask | ResumeTask | CancelTask)` with the actual actor preserved
and the saved task ID enforced. `CancelTask` is restricted to the empty-auction
early-cancellation policy above; it cannot be an unguarded generic forwarding
route. Third-party funding is allowed, but only the seller can resume or update
the task budget. A gas deposit does not resume, create a bid, or purchase assets.

`UpdateBudget` may change gas-per-poll and maximum gas price while preserving the
seller as refund recipient. After the first commitment, allow only nondecreasing
caps/ceilings. Do not expose host lease extension or recipient changes. No
controller promise can guarantee scheduling: a seller can still decline to fund
or resume, and expiry refunds remain the safety fallback.

Queries include immutable terms, height-derived bidding phase, business terminal
state, settlement cursor/totals, a single bid/claim, bounded bid pagination,
`FundingConfig`, `TaskFunding`, and a separate host-task view. Bound page size to
32 and do not expose a query that loads the entire auction implicitly. Show an
unavailable/pruned host view explicitly; business finality remains authoritative.

## Components and integration

- `contracts/native-batch-auction`: message/state definitions, pure allocation
  arithmetic, canonical commitment encoding, admission/reveal/claims, generated
  workflow, and native funding/registration bridge. Keep these concerns in small
  modules; do not add a general exchange or job framework.
- `crates/native-async`: N1/N2 and scoped funding-liability repair, with nested
  callback and admission regressions. Update reference host call sites for the
  trusted address API.
- `integration/wasmd`: matching resolver policy, differential fixtures, compiled
  auction scenarios, failure injection, node restart and state-sync scenarios.
- `formal/quint`: deliberately integrate existing tooling and add the auction
  abstraction plus regressions for the repaired host boundaries.
- Tutorial/client helper: commitment construction, reproducible local setup,
  bounded result queries, and receipts identifying source/artifact/node config.

Register task continuation bytes conservatively using the existing host quota
model; measure maximum generated checkpoint/envelope sizes. The application's
bounded bid records are ordinary contract storage, not part of that continuation.
Choose runnable gas caps from real-host measurements and record them as local
fixture calibration, not universal production prices.

## Required verification

1. **Runtime regressions:** reproduce N1/N2 and reserved-funds overflow boundaries
   before repair; compare Rust/Go accept/reject behavior and rollback/fee results.
   Preserve all existing DCA/native tests.
2. **Pure allocation and encoding:** independent one-shot reference oracle versus
   every batch size and interrupted phase; empty/underfull/exact/oversubscribed
   books; ties/dust; all-at-one-tick; maximum values; overflow rejection; exact
   binary commitment vectors and cross-domain replay rejection.
3. **Contract admission:** all C/R boundaries, repeated commitments/reveals,
   wrong funds, undercollateralized reveal, seller/beneficiary authentication,
   invalid attempts leaving buckets unchanged, and exact task registration.
4. **Transactional accounting:** successful/failed claims, duplicate claims,
   both coin payouts rolled back together, donations ignored, same-denomination
   gas versus principal, and two contract instances unable to spend one another's
   liabilities. Assert actual bank balances alongside accounting equations.
5. **Poll atomicity:** injected OOG/trap after partial batch writes restores the
   prior phase/cursor/records/checkpoint while charging consumed gas; explicit
   resume makes progress without repeated allocation or a duplicate payout.
6. **Terminal races:** finalization at E-1 versus abort at E, expiry in every
   partial phase, early cancellation only before commitments, no post-first-bid
   cancellation/budget reduction, and claims after host completion/pruning.
7. **Real Wasm/native chain:** bounded multi-block settlement with at least three
   nonempty batches, measured gas within each cap, price/funding pause and
   explicit recovery, no claims before finalization, and deterministic totals
   matching the independent oracle. Include N1's zero-price boundary independently
   of the example's normal positive-price configuration.
8. **Restart and state sync:** stop/restart after a nonempty batch; state-sync a
   fresh node at partial settlement and after finalized-but-unclaimed state;
   compare application results, task/funding state, eventual claims and balances
   with an uninterrupted run. Validate the new guard cannot survive restoration.
9. **Quint:** model sealed/revealed/frozen inputs, bounded cursor progression,
   provisional/claimable states, gas pause/resume, abort/finalize races, atomic
   failed polls, claims and restoration. Check collateral/inventory conservation,
   no premature or duplicate claims, terminal-state exclusivity, cursor safety,
   and at-most-one poll per block. Include mutation controls; publish exact finite
   bounds/seeds and keep arithmetic/unit/differential tests as separate evidence.
10. **Adversarial review and receipts:** independently review custody/rounding,
    authority, host gas/rollback and retained-state boundaries. Record exact
    commands, artifacts and residual risks; do not equate model passes with
    implementation proof or inherit old chain receipts for newly built artifacts.

## Runnable demonstrations

Ship a default happy path plus three deterministic failure/recovery scenarios:

- Multi-block oversubscribed clearing with a marginal rounding remainder and
  bidder/seller claims that match the independent oracle.
- Insufficient gas or price ceiling pause after partial allocation, followed by
  third-party top-up and/or seller ceiling increase as appropriate, no implicit
  resume, seller resume and correct completion.
- Restart after a committed batch and after finalization before claims; no
  recomputation changes entitlements and repeated claims fail.
- Settlement unable to finish before E; all bidders recover full collateral and
  the seller recovers inventory, with provisional allocations never paid.

Demo receipts distinguish runtime task state from business auction state and
record retained claims after host pruning. No public network, production funds,
deployment, merge or automatic PR publication is authorized by this specification.
