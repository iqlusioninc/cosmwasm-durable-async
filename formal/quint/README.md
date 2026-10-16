# Durable async Quint models

This directory owns its tooling; it adds no runtime dependency. The local
`@informalsystems/quint` version is pinned to **0.33.0** in `package.json` and
`package-lock.json`. It was exercised with Node **v26.11.1** and npm **11.20.0**.
The global Quint 0.31.0 installation is not used or changed. Node/npm versions are
observations, not enforced pins. The 2026-10-15 native-cache and batch-auction
extension is recorded in [auction-verification-results.md](auction-verification-results.md).
The unchanged [2026-10-12 receipt](verification-results.md) is historical evidence
for its named revisions, not verification of the extension.

## Reproduce the portable checks

From the repository root:

```sh
cd formal/quint
npm ci
./node_modules/.bin/quint --version
npm run parse
npm run typecheck
npm test
npm run verify
```

Equivalently, after installation, run `bash formal/quint/verify.sh` from the
repository root. It checks twelve model/test/profile files, six deterministic
suites, five seeded simulation profiles and eleven expected-failure controls.
It always selects the TypeScript evaluator. `npm test` runs the six deterministic
suites and twelve runner checks, including real auction mutation controls.
Parse/typecheck npm scripts cover all twelve files. No runtime Rust/Go suite is
invoked by these npm scripts. Auction exploration requires nonzero observations
for all four witnesses; missing/zero witnesses fail as a coverage gap.

The runner requires local pinned Quint 0.33.0 and rejects missing/wrong-version
tools, positive-check failures, and negative controls that fail for another reason.
Negative controls require **exit 1 and the expected assertion/invariant diagnostic**;
an arbitrary nonzero exit is a failure. Counterexample ITFs go into a new temporary
directory printed by the runner, or `QUINT_RESULTS_DIR` if explicitly supplied.
The pricing mutation is a separate fixture; no tracked model is edited during a run.

Portable mode prints `extended backend: NOT RUN` and makes no symbolic/exhaustive
claim. To request the restricted symbolic accounting check, provision an Apalache
0.62.1 server and its supported Java runtime separately, then run:

```sh
APALACHE_SERVER=localhost:PORT npm run verify:extended
```

Explicit extended mode requires a reachable supplied endpoint, then runs the
portable obligations and `accountingInit/accountingStep` at depth 1. Unavailable
backends and failed checks produce nonzero status; they are never counted as a
skipped proof. The preflight checks TCP reachability, not server version/protocol;
Quint performs the actual backend connection and check. Quint can attempt backend
startup if a listener disappears or fails its protocol check, so use a correctly
provisioned server and replace `PORT` with its listening port. No backend is
provisioned by the setup instructions above.
Observed machine-specific backend commands, versions and failed attempts are
recorded in [verification-results.md](verification-results.md). The consolidated
[all-PR review](../../docs/reviews/2026-10-12-all-open-pr-async-review.md) maps
source findings and implementation evidence to each pinned head.

The pricing-only test command is exactly:

```sh
./node_modules/.bin/quint test pricing_tests.qnt --backend=typescript --seed=1 --max-samples=1
```

These tests contain no nondeterministic choices. Seed 1 and one sample make the
invocation explicit and reproducible. Parse and typecheck include both pricing
files. The portable commands do not invoke symbolic or finite-state model checking.
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
the deterministic scenarios; `native_profiles.qnt` provides bounded profiles and
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
controller attempts, authenticated delivery/funding, and independently authorized
other-task management in between. An explicit `Guest` snapshot restores all task,
index and accounting mutations on failure; consumed poll fees and host meters are
outside that rollback. The explicit `flight` identity guards
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
costs. The controller's final reviewed-revision check also passed 147 Rust cases
on unchanged runtime source; this remains separate implementation evidence.

`fixedInit` models PR4's FIFO task order inside a round-robin contract ring,
preserving the ring head on insufficient block gas. `init` models PR5's
descending bid, then readiness height, then FIFO sequence order. Auction pending
waits reserve their due readiness height and sequence when staged; delayed
promotion preserves both, including across checkpoint restoration. PR4 Go timer
promotion instead allocates the current height and a new sequence. Auction
selection counts visits for inspections, scans past live eligible oversized
tasks, and stops at an ineligible bid. A selected expired entry settles immediately,
including an oversized expired auction entry. A separate bounded
maintenance pass pauses priced-out tasks after a one-height grace period using
the preserved `max(ready, last)` age. Changing a bid does not reset this age.
Maintenance may be skipped or exhausted, so no unconditional progress or
fairness property is asserted. The `visitCapContentionTest` shows two higher-bid
tasks consuming both visits over two blocks while the third never polls and
one gas unit remains unused each block. This is an allowed outcome, not a safety
violation or a quantified attack-cost result.

At an expired PR4 queue head, this model follows the Go host's gas-fit check and
ring rotation before immediate expiry settlement. Insufficient gas leaves the
expired task queued with escrow held until a later selection or maintenance visit.
PR4 Rust checks expiry first and can leave
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

## Historical source correspondence and current exclusions

The original reviewed source heads were PR4 `e783dbf` and PR5 `9ec0386`.
The N1/N2 findings below describe those historical revisions. Current Task 1/2
and auction Task 3/5 correspondence is recorded separately in the fresh receipt;
passing this model never substitutes for implementation regressions.
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
movement by callbacks. Nested funding/delivery and cross-task control are now
represented by the transaction snapshot, but arbitrary guest code, message ordering
and recursive dispatch are not interpreted. There is no proof of memory safety, native compiler
lowering, restart of a real VM, or production resource calibration.

## Reproduce native checks

From `formal/quint`, individual commands equivalent to the runner's native steps:

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
The controller reran this accounting fixture at reviewed revision `6c7a59c`:
Apalache 0.62.1 reported no violation at depth one (56,340 ms). This covers that
fixture's finish outcomes, including pending staging; it does not establish
changed expiry selection, delayed promotion, full restore or arbitrary-state
correctness. Historical receipts remain scoped to their original revisions.

The exact observed invocation is retained in `verification-results.md`; it used
Homebrew Java 21.0.12.1 because the system Java shim failed. Quint reused the
existing localhost:8822 Apalache server. Read-only
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
| Native typechecks and deterministic scenarios | All three files typecheck; 37/37 tests pass |
| Exact pricing suite | Fresh portable check: 28/28 pass; helpers unchanged |
| Auction seed 20261012, 1,000 traces, depth 80 | No safety violation; poll witness 552, accepted delivery 3, rollback 84 |
| Fixed seed 20261013, 500 traces, depth 80 | No safety violation; poll witness 249, accepted delivery 0, rollback 34 |
| Both negative controls, seed 1, depth 2 | Expected invariant violations at the first transition, exit 1 |
| Historical Apalache accounting fixture, depth 1 | Original relation only: no violation found, 105,392 ms, exit 0; runner receipt at `9cb9d2e` also predates F1/F2 |
| Reviewed `6c7a59c` accounting fixture, depth 1 | No violation found, 56,340 ms, exit 0; same narrow accounting boundary, no expiry/delayed-promotion/full-restore proof |
| TLC on native profile | Failed before state exploration on u64 literal; no checked-state count |

The final F1/F2 correction reran all nine typechecks, 95 deterministic scenarios,
all four seeded profiles and all six controls through the portable runner, plus
ten npm runner checks. The eight added native scenarios cover oversized expiry,
PR4 insufficient-gas deferral, reserved pending slots, delayed grace, reverse
maintenance FIFO, readiness-before-sequence order, restored delayed waits and
PR4 current-height promotion. The existing expiry case now checks settlement
through inspection in both profiles. The controller subsequently reran the full
extended runner at reviewed revision `6c7a59c`, including the narrow accounting
fixture above, and independently passed 95 deterministic cases plus ten npm runner
checks. The existing backend was left untouched. Full receipts and the completed
independent review are recorded in `verification-results.md`; broader transition
and source-refinement proofs remain outside the result.

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

## Legacy delayed services and IBC (PR1–PR3)

`service_waits.qnt` and `ibc_waits.qnt` are separate source-derived abstractions
of PR1 and PR3. Each exports `init`, `step`, `safety`, `correlation`,
`authorization`, and `retainedState`, plus pure transitions for deterministic
scenarios. Neither imports the native scheduler, auction, funding, escrow or
price model. Callbacks and public expiry require externally submitted
transactions; advancing height/time alone leaves a wait outstanding.

The service model follows PR1
[`demo-service`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/d555ae00919a01050770f47fbbd0dd498a33481e/contracts/demo-service/src/lib.rs),
[`fulfill`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/d555ae00919a01050770f47fbbd0dd498a33481e/contracts/fulfill/src/lib.rs),
the inherited
[`runtime`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/d555ae00919a01050770f47fbbd0dd498a33481e/crates/durable-runtime/src/runtime.rs),
and the
[`driver`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/d555ae00919a01050770f47fbbd0dd498a33481e/scripts/service-driver.py).
The deployment precondition fixes owner 1, immutable bound workflow 9, payment
service 2 and shipment service 3. `beginRequest` requires the bound workflow;
retained `(workflow, sequence)` tombstones prohibit duplicate requests even
after dequeue/pruning. Per-service request IDs order each queue independently.
Two workflows may share an order ID and overwrite its `paymentSeen` value.

`State.chain` separates application/runtime/queue state from the driver's
persisted `journal`, selected request and command. `ready` is ephemeral first
broadcast permission: restart discards it, while preserving an ambiguous
Intent or saved hash. Scope mismatch, transport ambiguity, missing hash and
unconfirmed/unindexed/zero-height receipts cannot automatically rebroadcast.
Confirmed success/failure or CheckTx rejection permits fresh queue/workflow
rediscovery. Driver preparation chooses Deliver, Expire or Prune from that
fresh state. Signing, transaction execution and receipt discovery are abstract
environment events, not an RPC/client implementation. The model abstracts one
serial worker and assumes its per-journal lock and filesystem persistence work.
The source writes/fsyncs a sibling temporary file, atomically replaces the
journal, then fsyncs its parent directory; no OS/power-loss experiment was run.

Service success consumes the current queue row and either starts shipment or
completes. A remote error commits terminal failure. An empty payment reference
commits both `paymentSeen=0` and application failure; empty shipment tracking
currently completes. Malformed/oversized results, VM aborts and downstream Begin
failure preserve the pretransaction chain state. Public expiry at the exact
deadline commits failure but retains the service row until a separate Prune.
Prune cannot remove a current wait merely because its deadline elapsed.
The `external` observation is a ghost for independent remote effects, not local
business storage; local expiry never reverses it. No real payment, shipping
proof, token movement, contract refund or signer-fee refund is modeled.

The IBC model follows PR3
[`ibc-query`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/1d8adb79847ddd764693fba891a64450d70cd6ca/contracts/ibc-query/src/lib.rs)
and its
[`protocol guide`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/1d8adb79847ddd764693fba891a64450d70cd6ca/docs/ibc-example.md).
Connection/version/order/port validation is abstracted by `connect(valid)`;
one channel binds permanently, and Close cannot enable rebinding. Endpoints 1
and 2 encode both channel and contract-port identity. Pending intents retain
application packet correlation/data, endpoints and timeout. Packet data is a
small representative for saved bytes: equality abstracts bytewise matching,
but distinct encodings of the same decoded value are not enumerated. The model
does not implement JSON canonicalization or unknown-field rejection. The nested shared Correlation
schema actually accepts unknown fields, despite the guide's broader wording.

IBC-core authentication is an explicit environment assumption. `callback`
requires an authenticated acknowledgement/timeout proof and a matching emitted
host commitment, including the chain-generated `packetSequence`. Consuming a
commitment prevents both ack and timeout from committing for that packet.
This abstract gate does not verify cryptographic proofs, relayers, consensus,
connection trust or real IBC replay handling. The contract's `matches` check
intentionally **does not compare packet sequence**. The direct-entrypoint
`adapterFixture` bypasses host admission, as Rust mocks do: changing only packet
sequence still advances through that fixture, while `callback` rejects it.
Public Deliver requires the actual self sender (9); a relayer address or a
claimed sender field is not authority. Valid self delivery is an internal part
of the atomic callback; normal `step` only attempts unauthorized public calls.

Source and remote clocks advance independently. At/after the source deadline,
an accepted callback expires the current wait regardless of acknowledgement
success. An authenticated early remote timeout removes pending intent and host
commitment but leaves runtime Waiting. Thus `Waiting => pending` is deliberately
not an invariant. A later local Expire can resolve it. Public local expiry does
not cancel the remote packet: its host commitment remains until an authenticated
late outcome settles at IBC core, without reactivating the local workflow.
Close retains outstanding waits, refuses new sends, and turns a predeadline ack
into terminal `channel_closed` failure. Remote errors and invalid acks also
commit failure. The honest-peer scenario produces `5 -> 10 -> 20`; `peer_lie`
shows a valid authenticated peer value 7 being accepted. Provenance alone does
not establish arithmetic truth or exactly-once external effects.

Both abstractions **assume SDK transaction/cache atomicity**. Dequeue/pending
deletion, application writes, continuation changes, self execution, outbound
messages and relevant IBC host commitment changes commit together or restore
the transaction snapshot on abort. Atomic single-step Quint transitions encode
that assumption; they do not prove an SDK cache or a Wasm engine implements it.
Restart preserves modeled durable state and drops service invocation permission;
there is no byte serialization, apphash, node-restart or state-sync proof.

## PR2 compiled-artifact correspondence and remaining obligations

PR2's
[`compiled.rs`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/f60a1ba85b94d221870265f7ecaca0405dc0d552/tools/vm-tests/tests/compiled.rs)
contains five implementation tests, freshly passed by the controller for the
rebuilt normalized `fulfill_example.wasm` SHA-256
`c01e95b82e68444a16b462ac75854f585c4e0000be38105707b8ec53b01e10a7`:

| Implementation fixture | Related abstract obligation |
| --- | --- |
| `compiled_workflow_restores_state_across_fresh_invocations` | Two committed waits and durable successor state across invocations |
| `unauthorized_and_stale_callbacks_do_not_mutate_storage` | Sender and exact current-sequence rejection |
| `deadline_boundary_and_application_failure_execute_in_wasm` | Exact deadline rejection/expiry; empty-reference application write commits |
| `callback_exhausts_vm_gas_when_budget_is_insufficient` | Distinguish VM abort from application failure; gas-abort branch is abstract |
| `report_callback_payload_costs` | Payload-cost observation only; no calibrated gas/size theorem |

The gas-depletion fixture deliberately discards the failed VM backend: it does
not establish host rollback. These mock-host VM tests do not discharge SDK
transaction atomicity or outbound-message execution. Quint interprets no Wasm
and proves no macro/compiler lowering, artifact identity, capability validation,
ABI compatibility, runtime metering correctness, gas calibration, full-width
overflow handling or arbitrary checkpoint encoding. These remain independent
implementation and host obligations, even when a bounded abstract run passes.

Separate fresh review evidence: PR1 executed 31 Rust integration-test executions
(including intentionally repeated baseline scenarios), plus 7 Python driver
tests; these are **not 31 newly added tests**, real signing, or SDK/VM execution.
PR3 passed 10 native contract tests, 6 focused public-API tests and 2 compiled IBC
VM tests. Its rebuilt current IBC artifact hash was
`1a1bd09bd659cda39560870e2add008942dcfafa51f066c2a1e091d83ded1dd9`.
The historical
[`ibc-smoke-result.json`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/1d8adb79847ddd764693fba891a64450d70cd6ca/docs/ibc-smoke-result.json)
names a different IBC artifact,
`e4bea19387c8971e3d7484f9c589d7c72a2039994d683481f872a86aa4c55116`;
its chain result is not evidence of a fresh chain run for the rebuilt hash.
Compiled IBC fixtures manually dispatch self messages and prove neither IBC
authentication nor SDK transaction rollback. These source-review results and the
controller's 147-case Rust check remain separate from formal-model evidence.

## Reproduce legacy checks and controls

From `formal/quint`, with pinned Quint 0.33.0 and explicit TypeScript backend:

```sh
./node_modules/.bin/quint parse service_waits.qnt
./node_modules/.bin/quint parse service_waits_tests.qnt
./node_modules/.bin/quint parse ibc_waits.qnt
./node_modules/.bin/quint parse ibc_waits_tests.qnt
./node_modules/.bin/quint typecheck service_waits.qnt
./node_modules/.bin/quint typecheck service_waits_tests.qnt
./node_modules/.bin/quint typecheck ibc_waits.qnt
./node_modules/.bin/quint typecheck ibc_waits_tests.qnt
./node_modules/.bin/quint test service_waits_tests.qnt --backend=typescript --seed=1 --max-samples=1
./node_modules/.bin/quint test ibc_waits_tests.qnt --backend=typescript --seed=1 --max-samples=1
./node_modules/.bin/quint run service_waits.qnt --backend=typescript --invariant=safety --witnesses sawAccepted sawRollback sawRestart sawAmbiguous --seed=20261014 --max-samples=500 --max-steps=80 --verbosity=1
./node_modules/.bin/quint run ibc_waits.qnt --backend=typescript --invariant=safety --witnesses sawAccepted sawRollback sawRestart sawEarlyTimeout --seed=20261015 --max-samples=500 --max-steps=80 --verbosity=1
```

These models bound workflow IDs to two non-reusable slots, sequences to two
waits, height/source/remote clocks to 0–6, and deadlines to current clock + 2
(at most 7 for a second wait). Service order IDs are 1/2, references 0/1,
per-service request IDs 1/2, external observations 0–2 and broadcasts at most 8.
IBC starts with value 0/5; honest responses double, dishonest responses supply
7, so modeled request/output values stay at most 20. Four possible packets
give host packet IDs 1–4. Abort/restart counters are unbounded verification
ghosts limited by trace depth. These are exploration bounds, not production
limits. Byte sizes, production active-count caps and u64 counter/deadline
overflow are abstracted by accepted/rejected outcome categories. No fairness,
unconditional expiry, driver availability or exhaustive state exploration is
asserted. The simulation relation explores multiple workflows and driver/clock
events; direct malformed-packet fixture calls remain deterministic tests.

| Final portable check | Observed result |
| --- | --- |
| All nine model/test/profile files parse and typecheck | Exit 0 |
| Deterministic pricing / native / service / IBC suites | 28 / 37 / 16 / 14 pass (95 total) |
| Service seed 20261014, 500 traces, depth 80 | No safety violation; accepted 136, rollback 165, restart 497, ambiguous journal 191 witnesses |
| IBC seed 20261015, 500 traces, depth 80 | No safety violation; accepted 57, rollback 81, restart 500, early timeout 26 witnesses |

Witness counts are traces containing the predicate, not transition counts or
coverage guarantees. No Apalache/TLC result is claimed for these legacy models.

All three controls below must exit **1 with `Invariant violated`**. They are
excluded from normal `step`; each failed at its first transition in the final
run with seed 1, one sample, depth 2. Full counterexamples are written locally.

```sh
./node_modules/.bin/quint run service_waits.qnt --backend=typescript --init=weakSequenceInit --step=weakSequenceStep --invariant=safety --seed=1 --max-samples=1 --max-steps=2 --out-itf=/private/tmp/service-sequence-negative.itf.json --verbosity=1
./node_modules/.bin/quint run ibc_waits.qnt --backend=typescript --init=weakHostInit --step=weakHostStep --invariant=safety --seed=1 --max-samples=1 --max-steps=2 --out-itf=/private/tmp/ibc-host-negative.itf.json --verbosity=1
./node_modules/.bin/quint run ibc_waits.qnt --backend=typescript --init=weakDataInit --step=weakDataStep --invariant=safety --seed=1 --max-samples=1 --max-steps=2 --out-itf=/private/tmp/ibc-data-negative.itf.json --verbosity=1
```

The service mutation accepts supplied sequence 2 while current sequence is 1,
violating `correlation`. The host mutation omits IBC admission and advances on
an unauthenticated callback, violating `authorization`; this demonstrates why
IBC-core authentication is an assumption Quint cannot discharge. The data
mutation weakens adapter intent matching in a **direct-entrypoint fixture**:
packet data 6 differs from saved 5, so the accepted receipt has `matched=false`
and violates `correlation`. Real host commitment authentication would reject
that altered packet before contract invocation. None of these controls is a
new runtime exploit or evidence that the reviewed source omits its checks.
