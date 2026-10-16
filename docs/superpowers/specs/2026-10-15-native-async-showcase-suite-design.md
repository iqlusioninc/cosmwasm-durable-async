# Native async showcase suite: architecture and delivery roadmap

Date: 2026-10-15

Status: written design approved on 2026-10-15. Stage A's Tasks 1–9 and independent
task/whole-branch reviews are complete; one cleanup re-review remains pending.
See the [plan](../plans/2026-10-15-native-batch-auction.md) and
[verification receipt](../../reviews/2026-10-15-native-batch-auction-verification.md).
This roadmap retains the boundaries and dependencies for later jobs, waitsets
and IBC stages, which are not delivered by the Stage A implementation.

## Intent and success criteria

Build a runnable reference suite for CosmWasm developers that demonstrates more
than scheduled transactions. A durable process should coordinate independent
work, survive restarts, account for its own execution costs, and recover from
partial success without repeating committed effects or losing custody records.

The user approved all four directions: multi-block auction settlement, awaitable
cross-contract jobs, bounded concurrent waits, and cross-chain withdrawal and
rebalancing. Runtime safety repairs are a prerequisite. DCA remains the smaller
introductory example. The previously proposed milestone escrow is not removed
from the backlog, but is not a dependency or an extra implementation hidden in
this suite.

The deployment target is local test networks and test tokens, including actual
two-chain IBC transfers. Production vaults, mainnet deployment, arbitrary route
selection, and production-readiness claims are outside this scope.

The suite's decisive demonstration is a failure after an intermediate effect
commits, followed by restart and correct recovery. A successful happy path alone
does not satisfy that requirement.

## Verified starting points

Open PR heads were refreshed on 2026-10-15. Preserve these stacks; do not assume
that independent branches have been merged or integration-tested together.

| Contribution | Pinned head | Role |
| --- | --- | --- |
| PR 7: native DCA | `5566d5bd3fd37a02fffc725c919f3980a1187b46` | Starting source for the new suite |
| PR 5: scheduler gas-price auction | `9ec038632388343738c72b60e5512d29d2669e66` | Native scheduler base inherited by DCA |
| PR 6: Quint and adversarial review | `0619cc691e2df77a9ee8ca117415aec10544a4b0` | Sibling formal/docs branch; integration still required |
| PR 3: IBC queries | `1d8adb79847ddd764693fba891a64450d70cd6ca` | Sibling transaction-driven IBC example; not native asset movement |

The current [native bridge](../../../crates/durable-runtime/src/native.rs)
supports generated sequential waits and height timers. The
[native ABI](../../../crates/native-async/src/execution.rs) has one wait and one
resolution per task. Existing durable child helpers embed their continuation in
the parent task; they do not create independently scheduled or funded jobs.
Generated terminal returns do not emit outbound messages.

The [existing review](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/0619cc691e2df77a9ee8ca117415aec10544a4b0/docs/reviews/2026-10-12-all-open-pr-async-review.md)
records N1 (zero-price in-flight exclusion), N2 (Rust/Go pending-wait acceptance),
and R1 (visit-budget priority/starvation risk). The formal branch is not a repair
for these findings. Revalidate source heads and findings at implementation time.

## Delivery structure

Use an example-led sequence. Building every runtime extension first would give
one shared API review but postpone executable application feedback. Building four
independent examples would permit more parallel implementation but duplicate
funding and result-delivery policies. The selected sequence uses the auction to
exercise existing primitives, then introduces shared job and waitset protocols
before composing the cross-chain example.

| Stage | Deliverable | Dependency and boundary |
| --- | --- | --- |
| A | Runtime safety repairs and multi-block sealed-bid auction | Existing DCA/native base; no new wait ABI |
| B | Independently funded, awaitable contract jobs | A's safety baseline; no concurrent waits required |
| C | Bounded waitsets and quorum-attestation example | A; reusable B service protocols, but not mandatory child tasks |
| D1 | Native scheduled IBC query bridge | A plus deliberately integrated PR 3 components |
| D2 | Cross-chain withdrawal with actual returned assets | B and D1; selected transfer rail and custody reconciliation |
| D3 | Rebalancing composed from verified withdrawal/deposit | D2; C when operations genuinely fan out |

Stage A's [written specification](2026-10-15-native-batch-auction-design.md) is
the first implementation unit. Each later protocol/runtime stage receives its
own concrete spec and implementation plan before coding. D1 can be investigated
in parallel with B/C after its design approval; the sequential withdrawal path
must not be blocked on concurrent waits unnecessarily.

Runtime fixes, the auction contract, and formal-model changes should remain
separately reviewable even when they share a delivery milestone. Do not merge,
rewrite, or publish existing PRs solely to simplify this roadmap.

## Shared contracts

### Execution and authority

- Continue using restricted async functions lowered into serializable
  continuations, not arbitrary Rust futures or persisted Wasm stacks.
- Poll through the privileged native entry point. A public callback authenticates
  and records an outcome; it does not recursively poll the task.
- Maintain at most one poll per task per block, bounded maintenance/dispatch,
  and atomic rollback of one failed poll while charging consumed execution gas.
- Preserve fixed identities, versioned workflow state, and authenticated
  correlations. Synthetic poll senders are never evidence of the original user.
- Cancellation stops local progression. It does not undo committed effects or
  establish that a remote operation did not execute.

### Funding and retained state

Every async-capable contract exposes `FundTask`, `FundingConfig`, and
`TaskFunding`, plus explicitly authorized resume/control routes. Credit requires
actual attached coins transferred into the identified task's host escrow.
Donations and caller-supplied numeric balances are not scheduler funding.

Parent and independent worker tasks have separate budgets, controllers, leases,
and refund ownership. Sponsoring execution never grants control or changes
payout recipients; topping up never implicitly resumes a paused task. Business
principal, gas escrow, and any result-publication budget are separate liabilities,
including when denominations coincide.

Host lease/pruning does not erase business obligations. Every example specifies
its own result, claim, intent, and replay-record retention. Retention is bounded
or explicitly persistent; host storage pricing is not falsely presented as
covering unlimited application records. Recovery must not require a pruned host
record to reappear.

### Liveness and observation

Eligible height is not guaranteed execution time. Preserve the current scheduler
priority policy and expose the known R1 risk; this suite does not introduce a
fair-service lane or claim unconditional completion. Examples need safe expiry
and custody recovery when progression never occurs.

Expose application state separately from host task state. A completed trade,
remote job, or finalized auction can precede or outlive host completion.
Unavailable host queries are not evidence of completion or cancellation.

## Stage B: awaitable contract jobs

Use a versioned job identity binding requester contract, parent task and wait
generation, provider, immutable request terms, and the provider's worker task.
The provider derives requester identity from the actual caller. A caller must
not choose an arbitrary callback target or supply a fabricated completion result.

The demonstration uses a delayed local service with an independently registered
worker task. Its parent awaits a typed, bounded, authenticated result. Execution
state and publication state are distinct, so a failed parent callback cannot
erase completed worker effects or force the underlying job to execute again.

Scheduler-driven publication is required. A permissionless manual delivery
transaction may be a recovery tool, but cannot be the normal path silently
substituted for autonomous progression. The concrete design must choose a durable
publication phase/outbox or a narrowly specified runtime facility, reserve its
gas, bound retries and retention, and handle a parent that is already terminal.
Merely returning a worker result is insufficient under the current bridge.

Cancellation propagation is explicit and best-effort where remote effects have
already committed. Results cannot be overwritten; duplicate requests and
publication retries use stable identities. Late results remain available for
business reconciliation even when a parent can no longer consume its wait.

## Stage C: bounded concurrent waits

The initial abstraction is one task with a bounded waitset, not a general-purpose
parallel executor. Start with homogeneous typed operations, ordered branch IDs,
one common height deadline, per-branch payload limits, and a total reserved-byte
limit. Support all-results and K-success quorum policies. Independently funded
jobs from B can serve as branches when their work spans multiple steps.

Install all subscriptions before emitting any messages. Correlations add a branch
ID to the owning contract/task/wait generation. Count success only after the
authenticated adapter validates its typed payload. A quorum-attestation example
requires distinct configured authorities; multiple slots alone do not constitute
independent votes or establish the truth of an attestation.

Quorum membership freezes when the Kth valid success commits in consensus
execution order; present returned members in branch-ID order. This is deterministic,
not a fairness or unbiased-oracle guarantee. A common deadline resolves remaining
branches explicitly. Duplicate/closed-branch authenticated callbacks need safe
idempotent handling so a late loser cannot roll back a completed fanout. Wrong
authority, wrong intent, and malformed data must not become accepted outcomes.

The concrete spec must cover impossible quorum, late messages after cancellation,
bounded tombstones, Rust/Go wire parity, and snapshot validation. Changes require
explicit ABI/storage-version handling that preserves active v1 tasks. Arbitrary
futures, unbounded fanout, heterogeneous combinator syntax, and general `select`
are not part of the first waitset increment.

## Stage D: native IBC and custody-aware composition

First port the pure-query example to native scheduling. Prove acknowledgement
delivery queues a resolution and a later block resumes execution without a
manual resume transaction. Packet timeouts and local height deadlines are
different clocks/decisions; do not convert seconds to blocks and imply equality.
Verify outbound contract-port binding in the real host. The current scheduler
dispatches returned messages with an empty IBC port, so direct packet dispatch
cannot be assumed to work without an adapter or host change.

The withdrawal example then uses two local chains, a fixed peer/vault, a single
selected asset route, and test tokens. A delayed vault is an explicit fixture,
not evidence of integration with real staking/unbonding. Reuse a pinned standard
asset-transfer rail rather than inventing an unaudited bridge. Its exact callback
or middleware integration belongs in D2's concrete spec.

Track three separate facts: command accepted, remote job completed, and expected
assets received into the intended custody account. Completion requires all facts
needed by the application, not just an acknowledgement string. Pin denomination
traces, amount constraints, beneficiaries, and peer identities.

A local timeout cannot authorize a refund of assets that may already have moved.
A proof-backed packet timeout settles that packet, not previously successful
commands. Retain a reconciliation ledger independent of the waiting task; late
financial facts must be recordable after local cancellation/expiry. Relayers and
authenticated external event submission are still necessary.

Only after withdrawal and deposit are demonstrated separately should D3 compose
them into rebalancing. Recovery may leave an explicit partially completed
position. No global rollback, automatic reversal, arbitrary DEX integration, or
cross-chain atomicity claim is allowed.

## Verification and evidence

Each stage includes transactional Rust tests, applicable Rust/Go differential
fixtures, compiled Wasm/native-host tests, and a runnable scenario with pinned
artifact hashes. Add real node restart and state-sync coverage at states where
the new feature retains partial progress. A process-local reconstruction alone
is not a state-sync test.

Extend the existing Quint models for the new transition boundaries and keep
negative controls. Model escrow/asset conservation, authenticated correlation,
immutable accepted results, at-most-one poll per block, finalization/claim gates,
and restored-state equivalence. Map model assumptions to source tests. Bounded
exploration or abstract atomicity assumptions do not prove Rust/Go/Wasm correctness.

Independent review covers custody, authorization, scheduling/gas, retained-state
restoration, and cross-chain ambiguity. Receipts distinguish source inspection,
model evidence, simulated execution, real-host execution, and live local network
observations. Existing PR evidence is not automatically inherited by new artifacts.

## Handoff

Stage A now implements the scoped native runtime repairs and auction, with
local verification and completed independent task/whole-branch reviews recorded
in its receipt. The final handoff still awaits one cleanup re-review. This is
not production-readiness proof, merge authorization or remote publication.
Later jobs, waitsets and IBC stages retain their own plans and design gates.
