# Native cache and batch auction verification — 2026-10-15

Status: model/tooling implementation and source correspondence completed against
reviewed Task 5 head `cf297483a257eda268fe70a138977869107494b2` and reviewed native
Task 2 head `efd13efb59c772b4875906e6474e0f1ee6ac6b31`. Controller confirmed the
Task 5 review gate on 2026-10-15. The native/auction abstractions are not a composed
refinement or implementation proof. This directory introduces no runtime dependency.

## Import provenance and baseline

Only `formal/quint/` was imported from PR 6 head
`0619cc691e2df77a9ee8ca117415aec10544a4b0`, by reading each Git blob and applying
scoped file patches. No sibling runtime ancestry was merged. Implementation
checkout base: `eb2bf5aac63fdb714e68b871b49da8dc3c798e7c`.
The original nine models, runner fixtures, package lock and
`verification-results.md` were preserved. The latter is an unchanged historical
receipt, including its old runtime findings and depth-one backend result.

Observed tooling: Node `v26.11.1`, npm `11.20.0`, project-local Quint `0.33.0`.
Only Quint is enforced/pinned. `npm ci` initially failed because the default
`/Users/zakimanian/.npm` cache was unwritable in this sandbox; retrying with
`--cache /private/tmp/auction-formal.00Ky3u/npm-cache` installed the locked tools.
The cache is not committed. No global tool or backend was changed. A repeated
locked install after verification also exited 0, retaining diagnostics in
[evidence/npm-ci.txt](evidence/npm-ci.txt): two high-severity dependency entries
and an unapproved protobufjs postinstall notice remain development-tool concerns.
No dependency update, script approval or audit suppression was performed.

Before extending any imported registered model, the unchanged commands
`npm run parse`, `npm run typecheck`, `npm test`, `npm run verify` all exited 0:
9 files, 95 deterministic cases (28 pricing / 37 native / 16 service / 14 IBC),
10 runner tests, 2,500 seeded traces and 6 exact-diagnostic negative controls.
Captured output: [evidence/baseline.txt](evidence/baseline.txt).
Fresh baseline ITFs were emitted under
`/var/folders/d8/q4n9h8vx5l14s40vhldysw2c0000gn/T/cosmwasm-quint-results-Mk7noh`.

Baseline witness counts: native auction 552 polls / 3 accepted / 84 rollback
of 1,000; fixed native 249 / **0** / 34 of 500; service 136 accepted / 165
rollback / 497 restart / 191 ambiguous of 500; IBC 57 accepted / 81 rollback /
500 restart / 26 early-timeout of 500. The fixed-native zero is an explicitly
retained random coverage gap, not evidence of accepted delivery in that run.

## Test-first evidence

- Native guest-cache tests: six new assertion failures against the unchanged
  native abstraction, then passing with the full cache snapshot. See
  [native-red.txt](evidence/native-red.txt). The existing flight-guard and initial
  overflow fixture already passed; they are not represented as newly reproduced
  implementation bugs. The overflow test now exercises the permitted in-flight
  funding path and the total-liability guard.
- Same-task immediate delivery needed an explicit staged pending response before
  outbound dispatch. Two assertions failed against the no-op staging transition,
  then passed for commit/rollback; [immediate-red.txt](evidence/immediate-red.txt).
- Restore initially overconstrained `due > last`. A retained accepted outcome
  followed by a failed poll at its deadline reproduced that model error. Restore
  now requires `0 < due < expiry`; admission still requires `height < due < expiry`.
  See [restore-red.txt](evidence/restore-red.txt).
- All 14 initial auction scenarios failed with QNT508 assertions against minimal
  no-op transitions, then passed. [auction-red.txt](evidence/auction-red.txt).
  A later partial-write test first failed because injection ran the full batch;
  injection now stops after the first item, before the other B-1 items.
- Required witness validation first failed its runner assertion, then passed.
  [runner-red.txt](evidence/runner-red.txt). Parser/backend errors are never counted
  as expected model failures; incidental reserved-name parse errors during
  development were corrected before collecting assertion-failure evidence.

## Domains and transition semantics

The auction random profile has three bidder/address classes and three ticks,
`p[t] = 1+t`, supply and candidate quantities 1..3, collateral per commitment
1..12, batch size 1..2, `C=2`, `R=4`, `E=20`, heights 0..21. Invalid/repeated bids
are rejected by normal guards. `L=1` and seller class 0 are fixed in random exploration; beneficiaries
are immutable classes 1..3. At worst B=1, `Pmax=3+2*3+4=13`, so
`R+Pmax+1=18 < 20`. The separate three-bid dust fixture uses supply 5 and
quantity 3 each; it is outside the first random profile. After review round 1,
`initialFor` also supports the separate exact Task 3 standard book: five bidder
classes, supply 5, L=10, reserve 2, tick step 1, three ticks, B=2. Revealed
(quantity,tick,collateral) are (2,2,8), (2,1,6), (2,1,6), (1,0,2), followed by
an unrevealed deposit of 7. The actual batch model must produce price 3,
allocations [2,2,1,0,0], refunds [2,0,3,2,7], sale payouts [20,20,10,0,0],
seller quote 15, seller sale 0, total collateral 29. Instance initialization
parameterizes only bidder classes, lot size and reserve; random `init` still
calls the original three-class/L1/reserve1 initializer. Creation-valid terms
remain an abstraction precondition; this does not expand the random profile.

`Book` stores frozen inputs, per-tick/total demand, phase, tick/bid cursors,
clearing data, provisional allocations, residual, running liabilities and claim
flags. `ScanPrice`, `AllocateBase` and `BuildClaims` visit at most B items; a poll
does not continue into a second phase. Finalization checks running totals in
constant work. Full-model invariant evaluation can fold all records; this is
verification work, not modeled application work. D<=S includes exact supply and
clears at reserve. Commit-order dust is applied once per eligible marginal bid.

For n marginal bids, the sum of real pro-rata shares is M; each fractional part
is below one, so integer residual `M-sum(floor(shares))` is less than n. If M<Dm,
every positive marginal quantity has floor allocation below its quantity; if
M=Dm, residual is zero. Thus the single residual pass suffices. The model checks
the resulting bounded examples and terminal zero-residual invariant; this
arithmetic argument is distinct from random trace coverage.

Poll failure uses a complete pre-transaction State snapshot, discarding partial
bid/phase/cursor writes while retaining one consumed gas unit and poll history.
Gas begins at 12, top-ups add 2 up to 26 deposited units, and funding never resumes
implicitly. Seller-authorized resume is represented as an allowed environment
action. Price scheduling itself remains in the separate native profiles.

Claims use immutable recipients, separate sale/quote balances and principal
liabilities, one bidder flag and one seller flag. A second-coin bank failure
restores the full outer snapshot, including a lazy abort. Abort is irreversible
at E for unfinished books, ignores provisional allocations, and does not depend
on a host record. Finalized claims survive expiry/pruning. Restart preserves all
durable application records/flags and discards the transient visit counter.

Native `Guest` snapshots include all task/funding records, indexes, queue/ring,
sequence, deposits, fees, refunds and guest effects. Poll history, meters and
consumed poll fees remain outside abort. Funding, authenticated delivery and
independently authorized other-task management may occur in-flight. `stagePending`
admits a wait before immediate outbound delivery while retaining the explicit
flight identity. Successful finish reloads the current cached task, preserving
nested deposits and a newly accepted inbox; abort restores the previous inbox,
continuation and accounting. Restored checkpoints omit both flight and staged
markers and validate canonical-address classes, not Bech32 bytes.

## Source correspondence ledger

These are inspected source anchors, not claims that this formal run executes
the source tests. The controller owns final implementation/regression evidence.

| Model transition / test | Inspected source / regression | Correspondence status |
| --- | --- | --- |
| `control`, `flight`, `finish`; zero/positive-price all-outcome guard cases | Task 1 `2d6c297b657effc1a9ba3db066dada27b0a0cbc4`: `runtime.rs` policy and funding management guards, poll cache marker insert/remove; `runtime_store.rs::flight_key/is_in_flight`; `zero_price_reentrant_controls_are_rejected`, `reentrant_outcomes_reject_controls_and_conserve_funding` | Source read; controller says reviewed. No runtime test executed by this task. |
| `fund`, `guestSnapshot`, `abortGuest`; nested funding and independent control | Same Task 1; `runtime.rs` reloads task and funding after cache commit; `nested_funding_preserves_reserved_liability`, `failed_nested_funding_rolls_back_and_releases_flight`; Go `TestInFlightFundingConservation` | Models transaction-wide rollback, not arbitrary recursive Wasm dispatch. |
| `fund` total liability guard | Same Task 1 `host.rs` checked available+reserved+deposit before transfer; `reserved_liability_overflow_rejects_before_bank_movement` | Uint128 boundary fixture; random native deposits remain <=24. |
| `stagePending`, `deliver`, `finish` | `immediate_callback_observes_staged_wait_but_only_runs_next_block`, `failed_outbound_discards_immediate_callback_and_retains_old_continuation` in native runtime tests | Explicit staged wait preserves accepted next-generation inbox on success and discards it on abort. |
| strict pending due checks; restore resolver/due policy | Task 2 `efd13efb59c772b4875906e6474e0f1ee6ac6b31`: Rust `Runtime::stage` receives trusted Api, validates canonical resolver and `h < due < expiry`; Go `validateResolver` shared by `stage`/`ValidateState`; Rust `shared_wait_admission_precedes_checkpoint_and_dispatch`, Go `TestWaitAdmissionSharedVectors`, `TestSnapshotWaitAdmissionFailsClosed`, `TestRestoreValidActiveV1AndRetainedOutcome` | Source read; reviewed controller head. Accepted old outcome can have due<=last/snapshot height; Go durable validation checks `due != 0 && due < expiry`. The model checks address classes, not Bech32 vectors. |
| base allocation, reserve pricing, entitlement | Task 3 `2e1bb9a60214bd7eebb88024bddb536b5582708d`: `math.rs::{validate_terms,base_allocation,entitlement}`, `state.rs::{SettlementProgress,BidRecord,AuctionRecord}`; tests `underfull_and_exact_supply_pay_reserve_even_when_all_ticks_are_high`, `standard_book_matches_independent_sorted_oracle_and_conserves_assets` | Source read; controller says reviewed/integrated. Big integer and encoding tests remain independent. |
| task registration / poll identity precondition | Task 5 `cf297483a257eda268fe70a138977869107494b2`: `contracts/native-batch-auction/src/contract.rs:136` reply checks auction ID, unbound nonzero task ID, host contract/program/expiry; `workflow.rs:58` checks saved task against correlation workflow ID; `settlement.rs:27` repeats task/Active/height checks. `tests/workflow.rs:540`, `tests/claims.rs:235` | Auction model assumes the correct authenticated bound task; it has no registration reply, workflow ID or cross-instance API. Source checks inspected, not formally established. |
| one phase, at most B visited tick/bid items; constant-work finalization | Same head `settlement.rs:47` one match arm; tick loop at70, bounded bid ranges at120/166, aggregate Finalize at225. `tests/workflow.rs:42` exact standard book for every B1..32 and `:81` phase/item checks | Actual model batch transitions correspond at business level. Model random B1..2, standard fixture B2; source supports B1..32. Item bound is not a VM gas bound or a bound on repeated storage reads. |
| positive delay and initial wait | Same head `workflow.rs:74` pre-R Continue with R-height; `:85` Continue only if height+1<E; `:107` SleepBlocks only inside Continue. `tests/lifecycle.rs:9` | Auction model suppresses pre-R polling (including its gas/checkpoint) and uses height/lastPoll guards for later progress. It does not encode SleepBlocks or generated continuations; source establishes positive delays. |
| unfinished E-1 commits batch, locks claims until E | Same head `workflow.rs:81` advances batch then returns LeaseWindowExhausted without another wait; `claims.rs:114` keeps Active claims locked before E. `tests/lifecycle.rs:87`; model `unfinishedLastHeightCommitsBatchLocksClaimsUntilExpiryTest` | Model `paused=true` coalesces stopped execution; actual host task is **Completed**, output LeaseWindowExhausted, business remains **Active**. This is NOT host-status equality: output encoding, completion gas refunds, and resume rejection are not modeled. No composed runtime refinement is claimed. |
| E-1 finalization, finalized entitlement permanence | Same head `settlement.rs:225` validates aggregates then Finalized/Done; `claims.rs:114` never converts Finalized to Aborted. `tests/lifecycle.rs:42`; model finalize boundary / finalized-never-aborts tests | Terminal business semantics correspond; source Finalized plus Done is represented by one Finalized phase. |
| helper failure, trap/OOG: outer rollback, retained checkpoint, consumed fee | Same head `contract.rs:183` converts generated PollOutcome::Fail to outer Err; `crates/native-async/src/runtime.rs:1247` cache, `:1287` commit-or-drop, `:1296` consumed-fee accounting, `:1311` pause without replacing old task checkpoint. `tests/workflow.rs:170`, `:213` | Auction poll snapshot/one gas unit abstracts this outer failure. Generic native application Fail commits effects; the auction wrapper deliberately changes it into transaction error. Error strings, byte checkpoints and VM metering are not modeled. |
| O(1) abort; claims independent of host/pruning; immutable destinations and atomic multi-coin send | Same head `claims.rs:105` flips one auction state with no bid scan; `:4/:57` claims consult local records, mark once, refund full principal on abort, use stored bidder/beneficiary/seller; `queries.rs:46` reports unavailable host independently. `tests/lifecycle.rs:204`, `tests/claims.rs:45/:102/:146/:214/:300/:325` | Model abort changes one phase; claim flag/payout snapshots and hostPresent=false abstract these business effects. No source storage-operation or chain-pruning proof; real bank atomicity is an assumption. |

Source/model representation details: source starts ScanPrice with an exclusive
tick cursor; model Open initializes the first scan, with `scan+1` corresponding
to the source cursor during scanning. Source separates base_allocated and
final_allocated; model allocated includes all base allocations plus processed
dust, so it is not the source final_allocated during partial BuildClaims. Terminal
totals correspond. The source also validates malformed stored terms, clearing
records and continuation envelopes that the model does not represent. The exact
five-bid standard fixture uses the same inputs/outputs as the committed source
fixture; the deterministic model case exercises actual commit/reveal/poll/claim
transitions, not merely an oracle. Unqualified Rust source names above are under
`contracts/native-batch-auction/src/`; `tests/` anchors are relative to that
contract directory, and full crate paths are explicit.

## Limits

This is seeded TypeScript simulation and deterministic model testing, not
exhaustive state exploration, a compiler proof, a Wasm execution, an economic
fairness result, or unconditional liveness. R1 visit-cap starvation remains an
allowed scheduler policy outcome. No Apalache server was requested/provisioned;
the optional 0.62.1 depth-one native accounting check is not run here and the
historical result does not transfer to changed models or the auction.

The native and application auction modules are separate abstractions, not a
composed refinement proof. SDK transaction atomicity, VM metering, authenticated
actor forwarding and durable-state integrity remain assumptions. No SHA-256,
salt entropy, UTF-8 encoding, Bech32 policy, byte schema/apphash, registration
reply authentication, Wasm continuation size, gas calibration, Uint256 overflow
exhaustiveness or multi-instance bank isolation is established by these bounds.
Auction creation is a precondition; early empty-auction seller cancellation,
budget policy, arbitrary donations and denomination coalescing are not explored.
Normal claims include bank-failure rollback, but the native model's final gas
refunds still assume successful payment. Real-chain restart/state-sync and
unpruned claim custody require separate implementation evidence.

## Initial handoff run (before review round 1)

From `formal/quint`:

```sh
npm run parse
npm run typecheck
npm test
QUINT_RESULTS_DIR="$(mktemp -d /private/tmp/auction-quint.XXXXXX)" npm run verify
```

All exited 0. Full output is retained in
[evidence/final-unified.txt](evidence/final-unified.txt).
12 files parse/typecheck; 125 deterministic cases (28 pricing, 48 native,
16 service, 14 IBC, 15 auction, 4 deterministic auction witnesses); 12 runner
tests; 3,500 seeded traces; 11 strict expected failures. No skipped test or
positive-check failure remains. Deterministic suites use TypeScript, seed 1,
one sample. The final partial-write regression red summary is retained in
[evidence/partial-write-red.txt](evidence/partial-write-red.txt).

| Profile | Seed | Samples / depth | Actual witness traces |
| --- | --- | --- | --- |
| Native auction scheduling | 20261012 | 1000 / 80 | poll 514; accepted 7; rollback 88 |
| Native fixed scheduling | 20261013 | 500 / 80 | poll 250; accepted **0**; rollback 46 |
| Service waits | 20261014 | 500 / 80 | accepted 136; rollback 165; restart 497; ambiguous 191 |
| IBC waits | 20261015 | 500 / 80 | accepted 57; rollback 81; restart 500; early-timeout 26 |
| Batch auction | 20261015 | 1000 / 160 | settlement **431**; abort **1**; rollback **788**; restart **1000** |

All report no invariant violation. Witness counts count traces, not transitions.
The single random abort observation is thin coverage, supplemented by explicit
deterministic abort-in-every-phase and abort-witness tests. Fixed-native accepted
delivery remains a random coverage gap; deterministic delivery/restore tests
are separate evidence. New stage interleavings changed native witness counts;
the unchanged baseline counts are not substituted for the final run.

The five auction mutation controls and existing five state controls each exited
1 with **exact stderr `error: Invariant violated`** and no unrelated error.
Pricing exited 1 with **`Error [QNT508]: Assertion failed`** and exact stderr
`error: Tests failed`. The runner rejects wrong status, signal, parser/backend
errors and mixed diagnostics. All state ITFs were written to
`/private/tmp/auction-quint.YQsVEF`; the auction files are retained under
[`evidence/itf/`](evidence/itf/).

| Auction control | Mutated behavior | Observed counterexample |
| --- | --- | --- |
| `weakProvisionalStep` | Allows claim from provisional BuildClaims | Premature payout/flag violates safety |
| `weakRepeatedStep` | Ignores prior bidder claim flag | Second payout violates once-only accounting |
| `weakExpiryStep` | Allows Finalize at E | Terminal-height safety false |
| `weakRollbackStep` | Commits first item of failed batch | Snapshot rollback safety false |
| `weakFlagsStep` | Loses persisted claim flags on restart | Paid totals disagree with restored flags |

Every control begins from a deterministic reachable prefix and fails at its
first mutation step (seed 1, one sample, depth 2). The runner separately tests
missing/zero/malformed required-witness results and verifies retained ITF files.
Optional Apalache: **NOT RUN**; no current symbolic result is claimed.

## Review round 1: discriminating boundaries, reachable aborts, exact standard book

The original frozen-book test used a full, already-revealed book. Its duplicate
and capacity guards masked missing closing-height checks. The original
every-phase abort test changed only a phase string on an unprocessed book and
therefore did not exercise provisional entitlements. Both review findings were
confirmed: the two old tests still passed when both closing-height guards were
removed and aborted payouts incorrectly used provisional allocations/refunds.

Replacements exercise an unused bidder below capacity at C, and a committed,
unrevealed, fully collateralized bid at R. Each is accepted immediately before
its boundary and rejected with the entire State unchanged at the boundary.
The abort test reaches Open, ScanPrice, partial AllocateBase, partial BuildClaims
and Finalize through actual polls. It asserts nonzero provisional allocation,
cursor and liability values in the relevant prefixes, then claims every bidder
and seller after E and verifies full collateral/inventory, claim flags, zero
remaining principal balances, conservation at each intermediate claim, and no
further settlement. Finalized-never-aborts is tested separately.

The strengthened tests fail with QNT508 for the temporary guard and abort
payout mutations. Bidder-refund and seller-inventory mistakes also fail in
isolation. All mutations were restored; these coverage repairs did not require
changing the normal boundary or abort transition. Full mutation outputs are in
[review-1-mutations.txt](evidence/review-1-mutations.txt).

The controller clarified that the Task 8 brief omitted the exact Task 3 standard
book. The old three-bid dust example was insufficient and is now accurately
named. `exactStandardBookWithUnrevealedBidTest` uses the economic terms/expected
outputs documented above through the shared batch model, with five bidder
classes and immutable L=10/reserve=2. It initially failed on the unsupported
instance, then failed on literal outputs after class capacity was added but
prices/lot sizes were not yet honored. Those red outputs are retained in
[review-1-standard-red.txt](evidence/review-1-standard-red.txt). The normal random
profile remains three bidders/L1/reserve1; no broader random exploration is
claimed. Tick step remains 1, three ticks, C2/R4/E20, and the standard fixture
uses B2 with sufficient existing gas for its eight settlement polls.

Fresh full commands `npm run parse`, `npm run typecheck`, `npm test`, and
`QUINT_RESULTS_DIR="$(mktemp -d /private/tmp/auction-quint-review1.XXXXXX)" npm run verify`
all exited 0: 12 files, **128 deterministic cases** (auction suite now 18),
12 runner tests, 3,500 traces and 11 strict negative controls. Full output:
[review-1-final.txt](evidence/review-1-final.txt). All seeds/depths and witness
counts exactly match the initial handoff table: auction settlement431 / abort1 /
rollback788 / restart1000; native fixed accepted remains0. Coverage limitations
and dependency warnings remain disclosed. Five fresh auction ITFs are retained
under [evidence/review-1-itf/](evidence/review-1-itf/).

No backend, runtime source, lockfile or historical receipt was changed. Task 5
source correspondence was pending at this review-round handoff; the subsequent
source correspondence section below supersedes that status, not these receipts.

## Final source correspondence and verification

The controller supplied reviewed settlement head
`cf297483a257eda268fe70a138977869107494b2` and confirmed independent Task 5
approval on 2026-10-15. Source inspection covers the ledger above, including
Task 4 admission/claims already present at that head. The live controller clone
advanced to `a2171e06f3659e5ba2e89e08a24e2eb27e8242e4` during inspection;
`git diff cf297483a257eda268fe70a138977869107494b2 -- contracts/native-batch-auction/src contracts/native-batch-auction/tests`
showed only conditional `entry_point` attributes. The mapping stays pinned to
reviewed cf29748; it does not certify later integration changes.

The added E-1 unfinished characterization reaches a real partial AllocateBase
batch, checks committed allocation/cursor and consumed fee, rejects bidder and
seller claims, permits no later settlement, then refunds full principal at E.
It first passed on existing transitions (exit0), failed with QNT508 after the
abort threshold was temporarily moved to E-1 (exit1), and passed again after
restoring the transition. This is a mutation-discriminating coverage addition,
not evidence of a newly found source bug. No normal transition changed.

Fresh commands from `formal/quint`:

```sh
./node_modules/.bin/quint test native_batch_auction_tests.qnt --backend=typescript --seed=1 --max-samples=1 --match=unfinishedLastHeightCommitsBatchLocksClaimsUntilExpiryTest
npm test
QUINT_RESULTS_DIR="$(mktemp -d /private/tmp/auction-quint-correspondence.XXXXXX)" npm run verify
```

All normal commands exited0; the temporary mutation alone exited1 with QNT508.
Full exact output: [source-correspondence.txt](evidence/source-correspondence.txt).
Fresh verification passed 12 parse/typechecked files, **129 deterministic cases**
(28 pricing, 48 native, 16 service, 14 IBC, 19 auction, 4 auction witnesses),
12 runner checks, 3,500 seeded traces and 11 strict negative controls. Seeds,
depths, domains and all witness counts equal the handoff table above. All controls
retained exact diagnostics; new ITFs are at
`/private/tmp/auction-quint-correspondence.pPhUer`. The five persisted review-round
auction ITFs remain valid unchanged-model artifacts. No optional backend ran.

Task 8 is complete with disclosed concerns: this is bounded model evidence plus
source inspection, not composed runtime refinement; dependency diagnostics
remain; random abort coverage is thin and fixed-native accepted coverage zero.
Task binding, generated initial waits/checkpoint encoding, actual E-1 Completed
host status/refunds, SDK bank atomicity and metering require the independent
implementation tests. The controller separately reported rerunning the source
exact-book all-32-batches and Fail-wrapper regressions successfully; those are
controller evidence, not commands executed by this formal task. Lockfile and
historical receipt remain unchanged, as does the normal auction model.
