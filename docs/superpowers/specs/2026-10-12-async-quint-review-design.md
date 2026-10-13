# All-open-PR async review and Quint model

Status: design approved; Tasks 1–3 implemented and independently reviewed.
Task 4 implementation is verified locally; final whole-branch independent review
is pending. Runtime findings remain report-only.

## Intent and scope

Build an executable formal model to support the adversarial review of every
currently open CosmWasm durable-async PR. Find short counterexamples, capture
source-to-model assumptions, and retain reproducible verification commands.
Do not equate model verification with a proof of Rust, Go, Wasm, IBC, or the SDK.
Runtime fixes, public review comments, PR updates, merging, and deployment are
outside this request unless separately authorized.

The user approved modeling registration/funding, waits and authenticated delivery,
bounded scheduling and pricing, rollback/charging, pause/resume/expiry, and snapshot
equivalence, then expanded both review and model coverage to all open PRs.

## Pinned review inventory

| PR | Head | Base | Coverage |
| --- | --- | --- | --- |
| #1 delayed services | d555ae00919a01050770f47fbbd0dd498a33481e | main | Request/result correlation, callback authority, retries, deadline, driver restart |
| #2 VM validation | f60a1ba85b94d221870265f7ecaca0405dc0d552 | main | Atomic guest execution, failure classification and artifact/host assumptions |
| #3 IBC waits | 1d8adb79847ddd764693fba891a64450d70cd6ca | feat/wasm-vm-validation | Channel/packet identity, ack/error/timeout, local deadlines, replay and closure |
| #4 native runtime | e783dbf83152842cfb211eda7554caaf3afd861a | main | Fixed-price scheduler, lifecycle, escrow, bounded work and restore |
| #5 auction | 9ec038632388343738c72b60e5512d29d2669e66 | feat/native-async-funding | Dynamic pricing, bid/FIFO selection, grace pause and cross-host behavior |

Recheck heads before final reporting. Review PRs at their own heads and review the
two existing stacks (#2 -> #3 and #4 -> #5). Do not merge unrelated PRs merely to
claim combined coverage. Record cross-stack integration assumptions explicitly.

## Architecture

Use small, composable Quint modules, not a monolithic model of the whole chain:

1. `pricing.qnt`: pure integer-atomics fee and next-price functions, the exact
   shared fixture cases including u128 saturation and sub-atomic truncation.
2. `native_async.qnt`: the native host state machine, configured for fixed price
   (PR #4) or auction policy (PR #5). Wasm is a nondeterministic, gas-bounded
   transition with success, pending, yield, application failure, and rollback
   failure outcomes. Separate guest effects from durable host accounting.
3. `service_waits.qnt`: caller/service and driver state, requests, authenticated
   callbacks, saved continuation/result and retry after process restart (PR #1).
4. `ibc_waits.qnt`: packet identity and authenticated ack/timeout transitions,
   channel closure, and local workflow deadlines (PR #3).
5. Small finite verification profiles/tests import these modules. PR #2's real
   VM tests provide evidence for the abstract guest transition, not a modeled
   Wasm interpreter or a formal proof of artifact equivalence.

Place models, pinned tool manifest/lock, tests, runner and coverage README under
`formal/quint/`. Keep audit reports under `docs/reviews/`. No runtime dependency
changes. Work on a separate local branch based on #5; retain independent PR source
heads as read-only review worktrees.

## Native state and transitions

State includes height/phase, task identity and monotonic sequence, ownership and
control, bid/cap/readiness, saved continuation/wait/inbox, status/pause reason,
expiry, escrow, paid lease/cleanup, fee collector/refunds, poll history, due/bid
membership, selection visits and maintenance budgets, block-start/next price,
execution gas, and explicit in-flight identity independent of money.

Actions cover register, positive third-party funding, authorized policy/lease
updates, explicit resume/cancel, authenticated result delivery, timer/timeout
resolution, expiry/pruning, start block, bounded maintenance and selection,
reserve/poll/commit-or-rollback/charge, finalize price and restart/restore.
In-flight callbacks are separate interleavings so reentrancy invariants are not
assumed merely by treating a whole poll as one atomic model step.

The model must preserve unresolved waits, accepted results, readiness order and
generation. Poll callbacks/yields enqueue no earlier than H+1; host timers become
eligible at H. Failure rolls back guest changes but retains prior continuation
and inbox and charges consumed gas. Application failure commits its effects.

## Properties and assumptions

Safety properties:

- Escrow conservation: deposits equal available/reserved funds plus charged
  execution/storage/cleanup fees and successful refunds; balances nonnegative.
- Task ownership isolation; contributors cannot acquire control or alter refunds.
- Only the recorded resolver/current wait generation accepts an operation result;
  accepted results are consumed at most once and survive failed polls.
- At most one poll per task per height; no expired/terminal/unresolved task polls.
- All polls use the block-start base; price never exceeds a polled task's bid;
  reservation and block gas budgets are respected.
- Bounded visits/maintenance even for skipped or priced-out work; same-height
  callbacks cannot recursively run a task.
- Pause does not erase continuation/inbox; funding alone does not resume; grace
  age uses preserved readiness/last-poll time rather than the last bid change.
- A checkpoint reconstructed from durable state has the same next-step behavior;
  queue/task identity mismatches and inconsistent accounting are rejected.

Service/IBC profiles additionally check authenticated correlation, replay
rejection, error/timeout distinction, and that local expiry does not reverse a
committed remote side effect. Restart drops process-local driver state, not chain
state. Contract-supplied actor/resolver fields rely on faithful forwarding by the
owning contract; model that trust boundary explicitly rather than assuming the
host authenticates the original user directly.

No unconditional liveness claim: strict bid priority, bounded maintenance and an
unresponsive external service permit starvation. Add explicit traces for visit
cap contention, absent response, retry and recovery. Any eventual-progress check
must state bounded demand, sufficient escrow/lease and fair scheduling assumptions.

## Verification strategy

Pin a project-local Quint version (0.33.0 candidate, subject to actual CLI
verification). The globally installed 0.31.0 fails with current Node; do not edit
global packages. Pin compatible Node tooling if required. Use parse/typecheck,
deterministic action-trace tests, seeded randomized simulation and bounded or
finite-state model checking where the available backend supports it.

Use small explicit domains for task IDs, actors, gas and heights so checks finish.
Publish every profile's bounds, seed, samples/depth, backend/version and exit code.
Exact pricing fixture tests retain real 18-decimal/u128 constants, separate from
small-state scheduler exploration. Artificial bounds are not production caps.

Include negative controls: deliberately weakened transition/property variants
must produce the expected counterexample; a green run alone is insufficient.
Validate source findings with focused Rust/Go/contract regressions where feasible.
Translate confirmed counterexamples into model scenarios without silently fixing
the implementation in the model. Label intended-spec versus implementation mode.

## Deliverables and limits

- Source-linked adversarial findings with severity, preconditions, minimal trace,
  observed versus inferred evidence, remediation and PR attribution.
- Runnable Quint modules, tests, tool lock and verification script.
- A coverage matrix mapping every open PR to modeled transitions, implementation
  tests and unmodeled obligations; explicit partial coverage is preferable to
  claiming entire-PR proof.
- A verification report separating simulation, bounded checking, exhaustive finite
  exploration and any unavailable backend. No proof of native memory safety,
  gas calibration, compiler lowering correctness, cryptography, network consensus,
  artifact equivalence or production readiness is claimed by the model.
