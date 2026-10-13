# Durable async Quint models

This directory owns its tooling; it adds no runtime dependency. The local
`@informalsystems/quint` version is pinned to **0.33.0** in `package.json` and
`package-lock.json`. It was exercised with Node **v26.11.1** and npm **11.20.0**.
The global Quint 0.31.0 installation is not used or changed.

## Reproduce the pricing checks

From the repository root:

```sh
cd formal/quint
npm ci --cache /private/tmp/cosmwasm-quint-npm-cache
./node_modules/.bin/quint --version
./node_modules/.bin/quint --help
./node_modules/.bin/quint parse --help
./node_modules/.bin/quint typecheck --help
./node_modules/.bin/quint test --help
npm run parse
npm run typecheck
npm test
```

The test command is exactly:

```sh
./node_modules/.bin/quint test pricing_tests.qnt --backend=typescript --seed=1 --max-samples=1
```

These tests contain no nondeterministic choices. Seed 1 and one sample make the
invocation explicit and reproducible. Parse and typecheck cover both pricing
files. These commands do not invoke symbolic or finite-state model checking.
The TypeScript evaluator handles the integer atomics without JavaScript `number`
rounding, as checked by the one-atomic and maximum-width fixtures.

CLI syntax is taken from this installed version's help. Upstream references:
[Quint CLI manual](https://quint.sh/docs/quint) and
[Quint source](https://github.com/quint-co/quint). The online manual can describe
features or defaults that differ from the pinned version.

## Exact pricing and domains

`pricing.qnt` models integer atomics at **10^18 atomics per whole price unit**.
Prices and floors have the Decimal u128 domain, maximum
`340282366920938463463374607431768211455`; gas, utilization, target and adjustment
denominator have the u64 domain. Quint integers provide wide intermediate
arithmetic, matching the Rust Uint256 calculation for these input widths.

`nextPrice(base, used, target, floor, denominator)` computes the magnitude in
this order, truncating after each division:

```text
magnitude = ((base * abs(used - target)) / target) / denominator
```

Increases saturate at u128 maximum. Decreases (including equality at target)
clamp at the configured floor, matching
[`next_base_price`](../../crates/native-async/src/pricing.rs).
Every `prices[]` row in
[`base-price.json`](../../fixtures/native-async/v1/base-price.json) has a test
with its exact literal expected atomics. The JSON's `bid_components[]` rows
describe key encoding, which is outside this arithmetic module.

`attemptCharge(gas, price)` computes `ceil(gas * price / 10^18)` with wide
integer arithmetic, matching the mathematical operation in
[`attempt_charge_at`](../../crates/native-async/src/host.rs). An oversized result
is not saturated: Rust's checked conversion rejects it. The maximum-width test
retains that wide result to expose overflow rather than hiding it.

Callers must guard with `nextPricePrecondition(...)` or
`attemptChargePrecondition(...)` before applying the corresponding operator to
runtime state. The price predicate rejects zero divisors and out-of-domain
inputs; the fee predicate also rejects a result above u128. The operators return
integers, not tagged Rust errors; invalid inputs are outside their contract.
No fallback price is assigned to a configuration with an invalid divisor.
Direct evaluation with a zero divisor is an evaluator error, not a modeled
rejection or a model-checker counterexample. State-machine callers must check the
precondition before evaluation.

These exact arithmetic tests are separate from bounded scheduler exploration.
Later scheduler profiles use small task/actor/gas/height domains for tractable
search; those bounds are not production caps. Passing these fixtures establishes
the modeled arithmetic examples, not a proof of Rust/Go execution or a complete
scheduler safety result.

## Tool dependency caveat

The pinned development dependency currently produces `npm audit` findings for
`adm-zip` through Quint (two high-severity dependency entries). No automatic
update or suppression is applied. The prescribed TypeScript tests operate on
local model text and do not download a Rust evaluator. In this version,
`dist/src/rust/binaryManager.js` uses `adm-zip` when extracting a downloaded
Windows `.zip` evaluator; macOS/Linux evaluator assets take its `.tar.gz` branch.
Do not interpret these development-tool findings as runtime crate findings.

## Native intended-semantics model

`native_async.qnt` exports `init`, `step`, `safety`, and the component predicates
`conservation`, `authority`, `polling`, `rollback`, `price`, and
`restoreEquivalence`. This is an executable **intended-safety abstraction**, not
a proof that the Rust or Go hosts implement it. `native_async_tests.qnt` contains
29 deterministic scenarios; `native_profiles.qnt` provides bounded profiles and
two separately named negative controls. Neither negative control is reachable
through the normal `step` or `tinyStep` relation.

The state has three non-reusable task slots, two actors/contracts, a height bound
of 6, poll caps of 1 or 2, block gas 3, visits 2, and a shared maintenance budget
of 6. Cumulative deposits stop at 24. Registration deposits 8: storage fee 1,
cleanup reserve 1, and execution escrow 6. Positive funding contributes 1;
lease extension contributes a storage fee of 1 and increases expiry by 1, at
most to height 6. Initial expiry is height 4. Timers/results distinguish accepted
delivery (inbox 1) from timer/deadline resolution (inbox 2). Sequence counters
and task generations are monotonic; task IDs cannot be reused after pruning.
The general relation is checked at bounded trace depths, **not exhaustively**;
the finite task/height bounds do not imply every allowed trace was explored.

The phase order is idle, maintenance, selection, optional closed selection,
then idle. Inspection enters selection and prevents further maintenance that
block. Reserve/start and finish are separate transitions, admitting same-task
controller callback attempts in between. The explicit `flight` identity guards
management even when the monetary reserve is zero. Success, yield, pending
operation/timer and application failure commit one abstract guest effect;
rollback/invalid wait outcomes preserve prior continuation and inbox and charge
consumed gas. The model does not interpret guest code or prove Wasm rollback.

The abstract guest transition **assumes SDK transaction/cache atomicity**:
successful guest effects commit atomically, while traps, invalid responses and
outbound failures discard staged guest writes/events; host charging/accounting
is then retained as modeled. It also **assumes `used` is trusted VM-metered gas
bounded by the reserved poll cap**, not a guest-supplied usage claim. Rejecting
an out-of-range `used` in Quint does not prove the real VM meter reports physical
work correctly or that real host work fits its reserve.

Supporting implementation tests are `TestAcceptanceRealVMFaultRollback` in
[`acceptance_test.go`](../../integration/wasmd/acceptance_test.go),
`TestRealWasmOutboundRollback` in
[`keeper_test.go`](../../integration/wasmd/keeper_test.go), and
`TestModuleEndBlockMeterIsolationAndRollback` in
[`boundary_test.go`](../../integration/wasmd/boundary_test.go). They exercise real
Wasm/SDK cache failure and gas-meter boundaries. These are separate implementation
test obligations; this model's simulation and bounded results neither execute
those tests nor prove SDK atomicity, VM metering correctness, or calibrated gas
costs. No implementation suite was rerun for the checkpoint-only fix.

`fixedInit` models PR4's FIFO task order inside a round-robin contract ring,
preserving the ring head on insufficient block gas. `init` models PR5's
descending bid/FIFO order. Auction selection counts visits for inspections,
scans past oversized tasks, and stops at an ineligible bid. A separate bounded
maintenance pass pauses priced-out tasks after a one-height grace period using
the preserved `max(ready, last)` age. Changing a bid does not reset this age.
Maintenance may be skipped or exhausted, so no unconditional progress or
fairness property is asserted. The `visitCapContentionTest` shows two higher-bid
tasks consuming both visits over two blocks while the third never polls and
one gas unit remains unused each block. This is an allowed outcome, not a safety
violation or a quantified attack-cost result.

At an expired PR4 queue head, this model follows the Go host's gas-fit check and
ring rotation before expiry handling. PR4 Rust checks expiry first and can leave
the same owner's live sibling at the ring head. That difference can change the
next inspection under a visit cap; the fixed model is not exact correspondence
for both hosts at this boundary.

For charging, small whole-unit prices are converted to atomics and passed to
the guarded Task 1 helpers. Price evolution uses the same integer recurrence
in a **scaled integer-price economy**, target gas 2, denominator 2, floor 1
(floor 0 for the zero-price profile). That scaling intentionally changes
rounding granularity relative to production Decimal prices. Production-width
fixture correctness remains the separate `pricing_tests.qnt` obligation.
`blockStartPriceAndUpdateTest` exercises two reservations at price 4 and a next
block price of 5; the ordinary base-1 simulation often remains at its floor.

`Checkpoint` is a distinct durable projection of live `State`. It carries task
records, height/config/next price, sequence, historical contract ring, redundant
ready/due record manifests, accounting totals and verification ghost poll history.
It excludes execution phase, current-block price/meters/cursor, in-flight context,
rollback cache, and live queue/index caches. Projection emits canonical manifests
from task records; restore validates those manifests against the decoded task
records, validates identity/generation/accounting and ring membership, then
reconstructs live queue/index caches. Only between-block checkpoints are accepted.
The fresh execution context has zero meters, empty scan/rollback caches and idle
phase. Ghost history preserves cross-restart checks; it is not a disk-schema claim.

`restartRebuildsQueueAndRingSuccessorTest` discards all live queue/index caches,
restarts from the durable projection, and requires the next selected task to be
task 3 under the saved ring `[2, 1]`. Replacing the ring with canonical owner order
would select task 2 and fail this test. `restoreNextStepTest` starts from unequal
live/reconstructed states and compares promotion/reservation/completion after an
accepted result; the inbox and continuation must survive. A separate regression
requires stale execution context to be cleared. `weakRestoreStep` deliberately
omits index reconstruction from a valid checkpoint: runnable task 1 cannot poll
in the next block and the safety predicate fails. The component name
`restoreEquivalence` still denotes metadata consistency; representative successor
tests add the behavioral correspondence checks. **No full bisimulation proof for
every Rust/Go next step is claimed.** Byte serialization, schema validation,
authentication and application-state hashes are outside the model.

## Source correspondence and exclusions

The reviewed source heads are PR4 `e783dbf` and PR5 `9ec0386`.
Relevant sources are [`runtime.rs`](../../crates/native-async/src/runtime.rs),
[`host.rs`](../../crates/native-async/src/host.rs),
[`scheduler.go`](../../integration/wasmd/scheduler.go),
[`snapshot_state.go`](../../integration/wasmd/snapshot_state.go), and
[`snapshot.go`](../../integration/wasmd/snapshot.go). The checkout contains PR5;
read PR4's own head for the fixed ring scheduler.

- N1: Rust's reservation-only management guard can permit an authorized callback
  when price is zero. `weakFlightInit`/`weakFlightStep` deliberately omit the
  independent flight guard and violate `authority`/`polling`. The safe model's
  rejection must not be reported as evidence that Rust already rejects it.
- N2: intended pending waits require `height < due < expiry`, following the Go
  acceptance boundary. Rust currently checks the lower bound but permits waits
  at/beyond expiry. The boundary test records the intended rejection and guest
  rollback; it is not a Rust regression result or a separate code-exact mode.
- R1: the visit-contention trace models an allowed priority outcome, separate
  from N1's implementation-discrepancy control.

Actor/controller/resolver integers abstract authenticated host forwarding.
This profile fixes controller and refund identity to the registering owner;
it does not explore separately delegated controller identities.
The contract must faithfully forward the original actor; no host authentication
of arbitrary contract-supplied actor fields is inferred. Address syntax,
byte-level queue encoding/cursors, stale-index repair, per-owner admission and
byte caps, host gas calibration, refunds that fail and fall back to a collector,
retention time before pruning, and schema/code/hash/manifest authentication are
not modeled. Refunds here always succeed and go to the immutable original
owner. Pruning retains a tombstone rather than deleting the record. Maintenance
chooses a task nondeterministically instead of reproducing every category's
lexical cursor; repeated no-op inspection can spend the shared budget.
Auction's scanned set abstracts the lexical cursor and does not model bid-key
movement by callbacks. During a poll, only same-task management attempts are
explored; cross-task reentrant writes, nested dispatch and transaction-wide
rollback of callback mutations are excluded. Funding/delivery interleavings
occur outside a poll. There is no proof of memory safety, native compiler
lowering, restart of a real VM, or production resource calibration.

## Reproduce native checks

From `formal/quint` (the package scripts currently cover pricing only):

```sh
./node_modules/.bin/quint typecheck native_async.qnt
./node_modules/.bin/quint typecheck native_async_tests.qnt
./node_modules/.bin/quint typecheck native_profiles.qnt
./node_modules/.bin/quint test native_async_tests.qnt --backend=typescript --seed=1 --max-samples=1
./node_modules/.bin/quint run native_profiles.qnt --backend=typescript --invariant=safety --witnesses sawPoll sawAccepted sawRollback --seed=20261012 --max-samples=1000 --max-steps=80 --verbosity=1
./node_modules/.bin/quint run native_profiles.qnt --backend=typescript --init=fixedInit --invariant=safety --witnesses sawPoll sawAccepted sawRollback --seed=20261013 --max-samples=500 --max-steps=80 --verbosity=1
```

The negative controls must exit **1 with `Invariant violated`**, not silently
pass and not merely fail to parse/typecheck. Save the full ITF counterexamples:

```sh
./node_modules/.bin/quint run native_profiles.qnt --backend=typescript --init=weakFlightInit --step=weakFlightStep --invariant=safety --seed=1 --max-samples=1 --max-steps=2 --out-itf=/private/tmp/native-flight-negative.itf.json --verbosity=1
./node_modules/.bin/quint run native_profiles.qnt --backend=typescript --init=weakRestoreInit --step=weakRestoreStep --invariant=safety --seed=1 --max-samples=1 --max-steps=2 --out-itf=/private/tmp/native-restore-negative.itf.json --verbosity=1
```

The first trace starts from a reachable zero-price in-flight task, cancels it,
and leaves `flight=1` with terminal status and `authorityOk=false`. The second
loads a valid durable checkpoint but deliberately omits rebuilding the discarded
indexes, leaving the ready task unreachable and `restoreOk=false`. It is a model
mutation to test reconstruction coverage, not an implementation finding or an
ability to forge a snapshot authenticated by a trusted application hash.

The `tinyInit` bounded profile fixes one pre-registered task, owner 1, bid/cap/base 1. Its
`tinyStep` explores begin/maintenance/reserve, yield/complete/rollback with gas 1,
finalization and validated restore. It excludes new registrations, external
funding/control/delivery and new waits. Depth 4 can reach reserve and completion,
but that attempted query was interrupted after several minutes without a result
(client exit 130). A prior depth-2 query passed in 74,089 ms; it is only shallow
scheduling evidence. Neither result covers the whole lifecycle or establishes
liveness.

The completed nontrivial bounded check uses `accountingInit`, constructed by
registering one owner-1 task with bid/cap 2 and base 1, beginning height 1, and
reserving 2 coins. `accountingStep` nondeterministically finishes with each of
six outcomes (yield, operation, timer, complete, application failure, rollback)
and consumed gas 0, 1, or 2; pending deadlines are height 2. Depth 1 checks this
boundary from that specific reachable fixture, not every possible poll state.

```sh
PATH=/opt/homebrew/opt/openjdk@21/bin:$PATH QUINT_HOME=/private/tmp/native-quint-backend ./node_modules/.bin/quint verify native_profiles.qnt --init=accountingInit --step=accountingStep --invariant=safety --max-steps=1 --verbosity=5
```

The local Java shim does not work; the path above selected Homebrew Java
21.0.12.1. Quint reused the existing localhost:8822 Apalache server. Read-only
process inspection identified its open Apalache JAR as version **0.62.1**, also
confirmed by that JAR's `version` command; the existing server was not replaced
or stopped. `QUINT_HOME` only controls downloads when a server is absent.
TLC was attempted with the same JAR, one worker, 1 GiB heap and 16 MiB stack.
It stopped before state exploration with `TLC can't handle a number this big:
18446744073709551615`. A conditional scaled-arithmetic branch still triggered
eager evaluation of the imported constant and was removed. This model's
production-width literals are unsupported by this TLC path; no exhaustive
TLC result is claimed. Apalache supports the imported integer widths.

Observed results with Quint 0.33.0:

| Check | Result |
| --- | --- |
| Native typechecks and deterministic scenarios | All three files typecheck; 29/29 tests pass |
| Exact pricing suite | Prior check: 28/28 pass; helpers unchanged by checkpoint repair |
| Auction seed 20261012, 1,000 traces, depth 80 | No safety violation; poll witness 552, accepted delivery 3, rollback 84 |
| Fixed seed 20261013, 500 traces, depth 80 | No safety violation; poll witness 249, accepted delivery 0, rollback 34 |
| Both negative controls, seed 1, depth 2 | Expected invariant violations at the first transition, exit 1 |
| Apalache accounting fixture, depth 1 | No violation found, 105,392 ms, exit 0 |
| TLC on native profile | Failed before state exploration on u64 literal; no checked-state count |

The checkpoint repair reran the native typechecks, 29 scenarios, both seeded
simulations and both negative controls. It did not rerun pricing helpers or the
symbolic backend: `accountingInit`, `accountingStep`, their live-state type and
the checked safety predicate are unchanged, and that relation never restores.
The prior depth-one accounting result therefore retains exactly its original
scope; it provides no new bounded evidence for checkpoint reconstruction.

Witness counts measure visited traces, not exhaustiveness. In particular, the
fixed-price simulation did not reach accepted delivery; deterministic scenarios
cover accepted delivery and retry separately. No Rust/Go test result is inferred
from these formal checks.

A redundant dedicated-port Apalache retry was canceled after the successful
shared-server result had arrived; it is not an additional verification result.
Its startup printed Apalache 0.62.1 build `a321771` and a warning about vulnerable
pre-22 protobuf generated code in gRPC reflection. That warning was not suppressed
or patched here. It is a separate development-backend dependency concern from
the npm `adm-zip` findings above; neither is a runtime contract finding.
