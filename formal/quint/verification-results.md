# Quint verification evidence — 2026-10-12

The current portable receipt includes the final F1/F2 model corrections after
whole-branch review of `da59262`: selected expiry settlement and preserved auction
wait readiness/FIFO. No runtime code changed. Results below separate fresh portable
execution from bounded backend and implementation evidence. Independent whole-branch
review and scoped final-fix re-review are complete; runtime findings N1/N2/R1 remain open.

## Reviewed-revision closeout

Whole-branch review of `9ec0386..da59262` found no Critical findings and two Important
model/source correspondence defects: F1 selected expiry settlement and F2 reserved
auction wait readiness/FIFO. Scoped independent re-review of `da59262..6c7a59c`
found both **ADDRESSED**, no new Critical/Important breakage and no new out-of-scope
observations. It inspected the fix diff, surrounding transitions, named source
anchors and regression/evidence receipts. This completes review of the formal/docs
deliverable, not a new runtime audit, runtime safety judgment or production approval.

The controller freshly verified reviewed revision
`6c7a59c534aaad710cffaa4315dcebe3adbe4363`:

| Check | Fresh result / receipt |
| --- | --- |
| `npm test` from `formal/quint` | Exit 0; 95 deterministic cases and ten runner checks; `/private/tmp/cosmwasm-adversarial.LDAo7r/final-reviewed-npm.log` |
| `cargo test --workspace --all-features --locked`, clean target | Exit 0; 147 Rust test/harness cases on unchanged runtime source; `/private/tmp/cosmwasm-adversarial.LDAo7r/final-reviewed-rust.log` |
| Full extended runner below | Exit 0; nine files parse/typecheck, 95 deterministic cases, 2,500 seeded traces and six expected controls, plus the narrow depth-one accounting check; `/private/tmp/cosmwasm-adversarial.LDAo7r/final-reviewed-extended.log` |

From the repository root:

```sh
APALACHE_SERVER=localhost:8822 QUINT_RESULTS_DIR=/private/tmp/cosmwasm-adversarial.LDAo7r/final-reviewed-itf bash formal/quint/verify.sh --extended
```

Apalache **0.62.1**, `accountingInit/accountingStep`, depth **1**:
`[ok] No violation found (56340ms)`; runner printed `extended check passed`.
The existing server was untouched. This is fresh evidence for this specific
accounting fixture and its finish outcomes, including pending staging. It is
**not** proof of changed expiry selection, delayed promotion, full restore or the
full native relation. Historical `9cb9d2e` receipts below retain their original
revision scope. Portable witnesses/bounds below remain unchanged.

The controller refreshed all five open PR heads at **2026-10-13 05:21:56 UTC**:

```sh
gh pr list --repo iqlusioninc/cosmwasm-durable-async --state open --json number,headRefOid,baseRefName --limit 100
```

All matched the pinned inventory. No push, merge or external PR update occurred.

## Fresh portable run

Observed environment: Node **v26.11.1**, npm **11.20.0**, project-local Quint
**0.33.0**, explicit **TypeScript** evaluator. No global tools changed.
From `/private/tmp/cosmwasm-quint.LDAo7r`:

```sh
cd formal/quint
npm run verify > /private/tmp/cosmwasm-quint-final-fix-portable.log 2>&1
npm test > /private/tmp/cosmwasm-quint-final-fix-npm-test.log 2>&1
```

The portable command exits **0**. All nine top-level `.qnt` files parse/typecheck.
Deterministic commands use seed **1**, one sample; simulation commands use
`--invariant=safety --max-steps=80 --verbosity=1` and the witnesses shown below.
The exact command arrays are committed in `verify.mjs`; equivalent individual
commands and semantic domains are in [README.md](README.md).

| Deterministic suite | Passing cases | Observed elapsed |
| --- | --- | --- |
| `pricing_tests.qnt` | 28 | 3 ms |
| `native_async_tests.qnt` | 37 | 60 ms |
| `service_waits_tests.qnt` | 16 | 10 ms |
| `ibc_waits_tests.qnt` | 14 | 9 ms |
| Total | **95**, none failed | No implementation suite inferred |

| Simulation | Seed | Samples / depth | Result, elapsed | Witness traces |
| --- | --- | --- | --- | --- |
| Native auction `init/step` | 20261012 | 1,000 / 80 | No violation, 6,812 ms | poll 552; accepted 3; rollback 84 |
| Native fixed `fixedInit/step` | 20261013 | 500 / 80 | No violation, 3,431 ms | poll 249; accepted **0**; rollback 34 |
| Service `init/step` | 20261014 | 500 / 80 | No violation | accepted 136; rollback 165; restart 497; ambiguous 191 |
| IBC `init/step` | 20261015 | 500 / 80 | No violation | accepted 57; rollback 81; restart 500; early timeout 26 |

Every profile exits **0**. Samples are independent seeded traces, not exhaustive
states or guaranteed branch coverage. The fixed simulation has no accepted-delivery
witness; deterministic scenarios check delivery/retry separately.

All six controls are **expected failures** checked by status and diagnostic:

| Family / control | Required observed failure | Meaning |
| --- | --- | --- |
| Pricing `runner-fixtures/pricing_negative.qnt` | Exit 1, `Error [QNT508]: Assertion failed` | Literal expected result is one atomic too high |
| Native `weakFlightInit/weakFlightStep` | Exit 1, `error: Invariant violated` | Reservation-only guard allows zero-price in-flight cancellation |
| Native `weakRestoreInit/weakRestoreStep` | Same | Valid checkpoint loaded without rebuilding derived indexes |
| Service `weakSequenceInit/weakSequenceStep` | Same | Wrong wait sequence accepted by mutation |
| IBC `weakHostInit/weakHostStep` | Same | Host authentication gate omitted |
| IBC `weakDataInit/weakDataStep` | Same | Direct-entrypoint intent/data check weakened |

All use TypeScript, seed 1, one sample; state controls use depth 2 and fail at
the first transition. The five state counterexamples are saved as ITF JSON under
the temporary directory printed by the runner. Controls are excluded from normal
`step`; they demonstrate check sensitivity, not new runtime exploit reproductions.

Focused runner verification: `npm test` runs all 95 deterministic cases and
`node --test verify.test.mjs`; ten runner checks pass, zero skipped. They use
real local CLI fixtures to reject missing CLI, invalid model, failed invariant
and parser errors masquerading as counterexamples, require correct diagnostic
and status, reject mixed counterexample and unrelated CLI/Node permission errors,
and reject unavailable explicit extended mode and unknown options.
The unavailable-server test reserves then closes a local ephemeral port; it
does not start/stop a backend. Fresh output is retained in the npm log above.

Focused TDD used `./node_modules/.bin/quint test native_async_tests.qnt
--backend=typescript --seed=1 --max-samples=1`: before the model fix, 29 passed and
eight failed with `Error [QNT508]: Assertion failed`; after the fix, all 37 passed.
These deterministic scenarios supply the new expiry/delayed-promotion evidence.
Unchanged seeded witness totals do not establish coverage of those new branches.

## Domains and proof limits

| Family | Checked domain / abstraction |
| --- | --- |
| Exact pricing | 10^18 atomics/unit; u128 price domain, u64 gas/target/denominator; unbounded intermediate integer arithmetic; oversized fee is rejected by caller guard |
| Native | Three non-reusable task slots, two actors/contracts, height ≤6, cap 1/2, gas 3, visits 2, maintenance 6, cumulative deposits ≤24; registration 8 = storage 1 + cleanup 1 + execution 6 |
| Native prices | Small whole-unit exploration economy, target 2, denominator 2, floor 1 (zero-price profile 0); not production Decimal rounding granularity |
| Legacy service | Two workflows/two waits, clock 0–6, deadline +2 (≤7), order IDs 1/2, per-service request IDs 1/2, references 0/1, external observations 0–2, broadcasts ≤8 |
| IBC | Two workflows/two waits, independent clocks 0–6, deadline +2, input 0/5, honest doubling or dishonest value 7, values ≤20, four host packet IDs |

Trace depth bounds unbounded ghost abort/restart counters. Finite data domains do
not make random simulation exhaustive. SDK transaction/cache atomicity, trusted
VM gas metering, authenticated sender/IBC proofs/commitments, faithful contract
actor forwarding, source clock and durable journal filesystem semantics remain
assumptions. No compiler/Wasm interpreter, cryptographic proof verifier, byte
serialization/apphash theorem, calibrated host gas, production cap/overflow
coverage, unconditional fairness, network consensus or production readiness is proved.
Native restore uses a separate durable projection, validates manifests/accounting,
rebuilds indexes and preserves ring history. Representative successor regressions
are checked; full Rust/Go bisimulation is not established.

## Historical extended runner and implementation receipts

The controller ran the explicit extended path at runner commit `9cb9d2e`:

```sh
APALACHE_SERVER=localhost:8822 QUINT_RESULTS_DIR=/private/tmp/cosmwasm-adversarial.LDAo7r/final-quint-itf bash formal/quint/verify.sh --extended
```

Exit **0**. The portable portion again passed 87 deterministic cases, four seeded
simulations and six checked negative controls. The runner connected to the
already running Apalache **0.62.1** server and reported `[ok] No violation found
(53603ms)` for `accountingInit/accountingStep` at depth **1**, then printed
`extended check passed`. Full output is retained at
`/private/tmp/cosmwasm-adversarial.LDAo7r/final-extended.log`. The backend
server belonged to another project and was not stopped or modified. This
exercises the runner's successful extended path on the same model/boundary as
the prior check; it is not a new restore, service/IBC or full-model proof. The
subsequent runner fix only tightened negative-control output classification;
no model or extended invocation changed at that time. The later F1/F2 correction
changes `inspect` and `finish` (including pending readiness allocation), as well as
promotion/order. The retained `9cb9d2e` depth-one result applies only to its original
accounting fixture/relation. The fix implementer did not start or rerun a backend;
the controller's later reviewed-revision accounting check is recorded above and
does not prove changed expiry selection or delayed promotion.

The controller also ran `CARGO_TARGET_DIR=/private/tmp/cosmwasm-quint-final-target.VlvP4z
cargo test --workspace --all-features --locked` from this formal branch. Exit
**0**: **147** Rust test/harness cases passed, none failed or ignored, with no
warnings in `/private/tmp/cosmwasm-adversarial.LDAo7r/final-rust-clean.log`.
An earlier attempt with the shared target directory exited **101** before tests:
`native_async::bid_component` and `native_async::next_base_price` were reported
as unresolved imports in `crates/native-async/tests/pricing.rs`. The source
exports were present and no runtime file changed; an isolated-target pricing
check then passed, followed by the full clean-target suite above. The failed
shared-target receipt remains at `/private/tmp/cosmwasm-adversarial.LDAo7r/final-rust.log`;
the clean retry supports a cache/target isolation explanation, not a source fix.

## Earlier bounded backend evidence

Quint 0.33.0; actual Apalache JAR/server **0.62.1**, Java **21.0.12.1**.
Task 2 observed this exact command from `formal/quint`:

```sh
PATH=/opt/homebrew/opt/openjdk@21/bin:$PATH QUINT_HOME=/private/tmp/native-quint-backend ./node_modules/.bin/quint verify native_profiles.qnt --init=accountingInit --step=accountingStep --invariant=safety --max-steps=1 --verbosity=5
```

Output: `Connecting with existing Apalache server`; `[ok] No violation found
(105392ms).`; exit **0**. Read-only process/JAR inspection identified the existing
localhost:8822 server, PID 83322, from `/private/tmp/lsm-lending`; it was not
stopped/replaced. This checks one reachable poll-settlement fixture, six outcomes
(yield, operation, timer, complete, application failure, rollback) × gas 0/1/2.
`accountingInit/accountingStep`, live-state type and checked predicate are unchanged
by the checkpoint repair that followed that check. The final F1/F2 wave subsequently
changed transitions used by that relation. The controller later rechecked the
narrow accounting fixture at `6c7a59c`, as recorded above.
That original boundary never restores; the retained result supplies **no new
symbolic evidence for checkpoint reconstruction or the final F1/F2 transitions**.

| Other backend attempt | Actual outcome / limit |
| --- | --- |
| `tinyInit/tinyStep`, depth 2 | Exit 0, 74,089 ms; shallow scheduling only, no completed poll |
| Same, depth 4 | Own client interrupted, exit 130, no result; not a pass |
| TLC via 0.62.1 JAR, 1 worker, 1 GiB heap, 16 MiB stack | Exit 1 before exploration: `TLC can't handle a number this big: 18446744073709551615`; no state count/exhaustive result |
| Transient scaled arithmetic workaround | Imported constant still eagerly evaluated; workaround removed |
| Redundant dedicated-port 8837 retry | Canceled; `Received RST_STREAM ...`, exit 1; no additional result; owned child/listener removed, shared 8822 left running |

No Apalache/TLC result exists for legacy service/IBC models. The historical extended
runner check above covers only its original native accounting depth one; the portable result
alone does not certify any symbolic check or omitted proof.

## Deferred development-tool concerns

The Task 1 `npm audit --json --cache /private/tmp/cosmwasm-quint-npm-cache`
exited 1, reporting **two high-severity dependency entries** (Quint via
`adm-zip`, and `adm-zip@0.5.18`). These are two package entries, not two distinct
advisories. Listed advisories: GHSA-xcpc-8h2w-3j85, GHSA-vwc7-r8mq-g2x9,
GHSA-7q85-xj36-vmfc, GHSA-rcw4-f5rp-g42v, GHSA-j5f4-cc29-5x44,
GHSA-p634-w6r4-rjp2, GHSA-c6fg-446q-cg94, GHSA-8238-w5pm-2374.
No update/override/suppression was applied. Quint's downloaded Windows evaluator
uses ZIP extraction; macOS/Linux assets use tar.gz. Portable TypeScript checks
process local model text and do not download evaluators.

Installation also printed npm's unapproved `protobufjs` postinstall notice.
The canceled Apalache startup reported build `a321771` and a vulnerable pre-22
protobuf generated gRPC reflection warning. These remain unresolved development
tool/backend concerns, separate from runtime-contract findings. No new audit or
backend startup was performed in Task 4.
