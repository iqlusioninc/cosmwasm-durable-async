# Native batch auction verification receipt

Date: 2026-10-15. Tasks 1–9, their required verification and independent task
reviews are complete. Independent whole-branch adversarial review is complete;
its scoped publication, instrumentation naming and status cleanup is implemented
and independently approved. All three cleanup findings are addressed with no new
breakage. Stage A's local implementation and review gates are complete. No push,
PR, merge, public deployment or production readiness is claimed.

Task 9 review base: `289da04424d7ac20a1da810cc108e2d2366a4664`.
Executable verification source: `e39d3792f18e5001f1e51469c6ff4ca0766747bd`, clean
when every final demo began. Original verification checkout:
`/private/tmp/auction-demo.zmkQ4a`. That historical executable-verification source
remains on the preserved local `feat/native-batch-auction` implementation branch.
The subsequent documentation and test-instrumentation naming cleanup changes
no production Rust, Go or formal source from cleanup base
`38a8aeb818b6e1f5317f39fadb1e4d572f0d32b7`.

The separate local `feat/native-batch-auction-ready` handoff starts with the
squashed implementation commit `453ffc9c159cde4f3bbb4f4bcf61e4400b89d5aa`, parented
directly at approved plan base `eb2bf5aac63fdb714e68b871b49da8dc3c798e7c`
(`docs/async-showcase-suite`). Its tree is identical to preserved implementation
cleanup commit `034a89f52ab4b303bb80f82d950335590bf438ad`; the subsequent completion
record changes only documentation.
Task 6/7 controller reports remain available locally but are excluded from its
tree and complete reachable history. The detailed implementation branch is
preserved without rewriting it. Historical source identities in this receipt
identify local evidence; no claim is made that those old commits are public.
If publication is later authorized, publish only the ready branch, never `--all`
or the preserved implementation branch containing historical controller reports.

## Evidence layers

| Layer | Fresh result and boundary |
| --- | --- |
| Rust/source | 269 tests across 43 groups; format and strict workspace/all-target/all-feature Clippy passed. Covers native safety, allocation/encoding, admission, custody and bounded workflow source; not an actual VM gas measurement. |
| Real Wasm/app | All five auction acceptance tests passed in the full Go run and an additional stable-artifact run (11.988s). Genuine post-write error/trap/VM OOG rollback, actual second-transfer failure rollback, real gas/funding and bank payouts were checked. |
| Disk reopen | Fresh app/keepers/VM against the same disk at partial and finalized-unclaimed state matched all 31 committed roots with uninterrupted replay. Separate from process restart. |
| Process | Auction network tests passed restart after AllocateBase, BuildClaims and finalization. The runnable restart demo independently reopened at H23 and H29, with new PIDs and exact committed captures before handshake/replay and next-block release. |
| Actual snapshots | Fresh empty joiners restored authenticated H23 and H29 snapshots from two independent sources, compared exact-H state before H+1 release, progressed and claimed after pruning. Invalid due==expiry restore rejected and retained offending bytes/activation marker. |
| Quint | 12 files, 129 deterministic cases, 3,500 seeded traces, 11 expected negative controls and 12 runner tests passed. These bounded abstractions and source correspondence are not a Rust/Go refinement proof. |

The normal auction artifact is used by every runnable scenario. Fault controls
exist only in the separate acceptance artifact. Native zero-price dispatcher
regressions are separate from the positive-price auction fixture; generic state
sync and DCA restart successes are not substituted for auction-specific evidence.

Earlier reviewed evidence retained from Tasks 6/7 is local source
`62a5d8fc87189bc4f1ae24cd8ecc75482e3a06fa` (real-host acceptance, based on
settlement `cf297483a257eda268fe70a138977869107494b2`) and
`289da04424d7ac20a1da810cc108e2d2366a4664` (process/snapshot recovery, based on
`d2af86e5bf5f97e8220799611ca41b83c3d296c7`). Task 6's exact shared-book acceptance
passed in 4.788s, full Go in 65.226s (120 top-level passes), with separate
zero/positive-price funding/exclusion cases. Retained logs are
`/private/tmp/auction-{acceptance,go-full,zero-price}.log`. Measured normal polls
consumed 198,001–284,116 SDK gas; injected VM OOG consumed 30,000,000 execution
plus 37,400 host = 30,037,400 total, costing 30,038 ugas at .001. These fixture
measurements do not bound all permitted books or production costs.

Task 7's named process/snapshot suite passed in 284.163s: all three restart
states (H23 AllocateBase, H26 BuildClaims, H29 finalized-unclaimed), two actual
snapshot states (H23/H29) and the separate invalid-wait rejection. The legacy
network regression passed in 19.252s. Logs remain
`/private/tmp/auction-recovery-{named,legacy-network}.log`. The local network uses
500ms TimeoutCommit, one-second RPC timeouts and 150-second scenario deadlines;
providers progress while the joiner is checked at fixed authenticated H, with
signed H/H+1/H+2 headers, exact-H capture and an acknowledged H+1 gate. It is not
public-network performance or unconditional liveness evidence. The later fresh
Task 9 runs and snapshot identities below remain separate integrated evidence.

## Commands and results

Observed tools: Rust 1.99.0 (`b940084d7`), Cargo 1.99.0, Go 1.23.6 darwin/arm64,
Node 26.11.1, npm 11.20.0, local Quint 0.33.0 and checked Binaryen 123.
The reused patched modules are wasmd v0.55.1 and wasmvm v2.2.4; the retained
workspace selects this checkout's adapter, not a stock wasmd executable.

```sh
export GOTOOLCHAIN=go1.23.6
export GOPATH=/private/tmp/auction-go.LW7Ijl
export GOCACHE=/private/tmp/auction-go.LW7Ijl/build
export GOWORK=/private/tmp/auction-demo-workspace.ZuMJIt/go.work
export CARGO_TARGET_DIR=/private/tmp/auction-demo-rust.8VtSNJ
export NATIVE_AUCTION_OUTPUT=/private/tmp/auction-demo-final-receipts.bZHo37

bash contracts/native-fulfill/build-wasmd.sh
bash contracts/native-dca/build-wasmd.sh
bash contracts/native-batch-auction/build-wasmd.sh
cargo test --workspace --all-features --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

NATIVE_ASYNC_NETWORK_TEST=1 NATIVE_AUCTION_NETWORK_TEST=1 \
  NATIVE_DCA_DEMO=1 NATIVE_DCA_SCENARIO=all \
  NATIVE_AUCTION_DEMO=1 NATIVE_AUCTION_SCENARIO=all \
  NATIVE_AUCTION_OUTPUT=/private/tmp/auction-demo-final-receipts.bZHo37 \
  NATIVE_DCA_OUTPUT=/private/tmp/auction-demo-final-receipts.bZHo37/dca-full \
  go test -C integration/wasmd -tags upstream -count=1 -timeout 20m -v ./...

# Run with no concurrent builders/host readers in this checkout.
bash scripts/native-auction-demo.sh happy
bash scripts/native-auction-demo.sh funding
bash scripts/native-auction-demo.sh restart
bash scripts/native-auction-demo.sh expiry
bash scripts/native-auction-demo.sh all

# After the final builder, with stable pre/post SHA manifests:
go test -C integration/wasmd -tags upstream -count=1 -v -run '^TestAuctionAcceptance' .

(cd formal/quint && npm ci --cache /private/tmp/auction-demo-npm-cache && npm run verify && npm run test:runner)
node --test scripts/auction-commitment.test.mjs
bash -n scripts/native-auction-demo.sh scripts/native-auction-demo-test.sh
bash scripts/native-auction-demo-test.sh
bash scripts/native-dca-demo-test.sh
git diff --check
```

All listed commands exited 0. Full Go: 620.004s, 128 top-level passes, zero
failures. Only the two intentionally inactive standalone cleanup-child
entrypoints skipped; their owned-cleanup tests actually launched them and passed.
Every requested behavioral opt-in ran: auction demo 4/4, auction restart 3/3,
auction state sync 3/3, existing DCA demo 5/5 and existing network state sync.
Auxiliary patch-command packages with no test files are not counted as passes.

Public runner commands each rebuilt their auction artifacts, executed actual
nodes, and verified fresh invocation-specific receipts:

| Selection | Go package time | Receipt directory under the output root |
| --- | ---: | --- |
| happy | 60.650s | `run.Euc13s/happy-3462895962` |
| funding | 61.132s | `run.RfVYmA/funding-534137821` |
| restart | 63.772s | `run.ncZNQH/restart-1396918496` |
| expiry | 54.399s | `run.pk7VFL/expiry-362348986` |
| all | 224.224s | `run.h2zTNx/` with all four scenarios |

Output root: `/private/tmp/auction-demo-final-receipts.bZHo37`. Every receipt
records source `e39d379...`, `source_dirty=false`, exact normal artifact, live
funding/node config, signed transactions, business/host snapshots and owned
child exits. Restart receipts also record old exited PIDs and exact startup
captures. The full Go run separately retained all four demo receipts under
`native-auction-run-1818277357/` in this same output root.

Logs are `/private/tmp/auction-demo-{happy,funding,restart,expiry,all}-final.log`,
`auction-demo-go-full.log`, `auction-demo-stable-acceptance.log`,
`auction-demo-rust-{test,fmt,clippy}.log`, `auction-demo-build-{fulfill,dca,auction}.log`,
`auction-demo-quint-{verify,runner,install}.log`, `auction-demo-node-final.log`
and `auction-demo-{shell,legacy-shell}-final.log`, all beneath `/private/tmp/`.

## Outcomes and limits

The exact shared book deposits collateral `[8,6,6,2,7]`, total 29. Final price
3 gives allocations `[2,2,1,0,0]`, sale payouts `[20,20,10,0,0]`, refunds
`[2,0,3,2,7]` and seller quote 15. Happy checks three nonempty allocation and
three claim-building batches, locked principal during work and real payouts
after host pruning. Early and duplicate claims produce actual failed DeliverTx
receipts (code 5), with unchanged business claims and sale/quote balances.

Funding uses a local 500-ugas margin beyond one maximum reservation. Initial
timer/scan and the first allocation consume 716 ugas, leaving 99,784 below the
100,000-ugas next reservation. It pauses at H24/cursor2; sponsor adds 1,000,000
ugas at H26 and seller raises the ceiling at H28, both still paused; explicit
seller resume at H30 leads to finalization H36. This is `insufficient_funds`,
not an organic price-limit experiment. Actual price-limit recovery is separately
covered in acceptance. The attempt cap is 100,000,000 SDK gas with 70,000,000
host reserve, and continuation allocation 17,920 bytes. These are local fixture
calibrations, not worst-case cost bounds for every permitted book or production
prices. B bounds distinct bid/tick records per poll, not exact storage reads.

Restart checks partial H23 and finalized-unclaimed H29 before releasing the next
block. Expiry E77 rejects provisional claims at H76, shows the host absent at
H78, lazy-aborts and pays full collateral/inventory by H90. Finalized claims
stay finalized after pruning; aborted claims ignore provisional allocation.
The auction remains capped, has no non-reveal penalty or Sybil resistance, and
promises no unconditional scheduling, service or economic fairness.

Actual full-run snapshots (format 3, one chunk each): H23
`0B0B41C56334215C69C06CECD42E7F2AE4B6E3F4123660783AFD1C05ECB67AD3`, H29
`235A409E67D6A88F38E82AA0762AE532BFE81D0468A337BEFF556F05DD1BA353`.
Their joiner PIDs 9059/15006 started history at 24/30. Full roots, authenticated
trust hashes and metadata identities are retained in the full Go log; genuine
Comet times make these receipt identities vary across independent runs.

Quint's auction profile uses seed 20261015, 1,000 traces, depth 160, three bidder
classes/ticks, supply/quantity 1..3, batch 1..2 and C/R/E=2/4/20; the exact
five-bid book is a separate deterministic fixture. Required auction witnesses
were nonzero (settlement 431, abort 1, rollback 788, restart 1,000). Existing
native/service/IBC profiles add 2,500 depth-80 traces at seeds 20261012..20261015.
The fixed-native random profile witnessed zero accepted deliveries in 500
traces; deterministic tests are separate evidence, and that coverage gap remains
explicit. See [model bounds/correspondence](../../formal/quint/auction-verification-results.md).
No extended Apalache or Rust-evaluator result is claimed. npm retains two high
package entries, Quint and transitive `adm-zip` 0.5.18, plus a separate unapproved
`protobufjs` install-script notice. The inspected portable TypeScript path avoids
the Rust runner's ZIP bootstrap extraction. This is disclosed development-tool
debt, not a general safety proof or dependency remediation; no upgrades or
install-script approvals were performed.

## RED/GREEN and artifact concurrency disclosure

The shell runner first failed its missing-entrypoint test. A later actual shell
regression showed zero-exit missing/skipped tests could masquerade as success;
fresh per-invocation indices and exact receipts now reject them, including
planted valid stale receipts in the same existing output directory. Shell stubs
prove path/env/evidence handling only, separately from the real runs above.

The first real happy attempt finalized but failed the literal oracle because
parallel first broadcasts reordered dust-sensitive commitments. Ordered first
broadcasts with concurrent lookups corrected it. Removing that order guard and
replacing exclusive salt creation with truncation each made the corresponding
safety test fail, then pass after restoration. Generic journal/deadline/owned
cleanup tests characterize existing helpers; they are not a new runtime repair.
Failure and mutation logs remain retained; no successful model run substitutes
for these implementation checks.

An initial scheduling mistake overlapped the first three public auction
builders with the full Go suite. wasm-opt writes normal/fault destinations in
place. Full Go began at 22:24:15 -0700; those public build logs began 22:24:15,
22:25:43 and 22:27:31. Last overlapping normal/fault mtimes were 22:27:50 and
22:28:11. The first overlapped earlier tests/acceptance, the next happy/funding,
and the third restart/expiry; earlier exact write instants were not retained.
No resulting test failure occurred. All other six artifacts were unchanged;
auction node/demo setup checks the exact normal SHA at load time.

Only the owned outer runner-loop shell was held between scenarios; its active
child and deadlines continued normally. After full Go exited, remaining
builders ran serially. Once the final builder finished, the additional fresh
five-test acceptance run passed in 11.988s with byte-identical pre/post hashes
for all eight artifacts. Manifests:
`/private/tmp/auction-demo-stable-artifacts-{before,after}.sha256`.
This mitigates unbound normal/fault acceptance reads without hiding the original
overlap or rerunning the entire network matrix solely for potential overlap.

For reproduction, serialize every builder/public demo script with host/network
tests or demos reading that checkout's artifacts, or use separate clones with
separate artifact directories. `CARGO_TARGET_DIR` alone does not isolate the
published files. This is observed harness resource coupling, not a runtime bug;
no normalizer rewrite is included.

## Fresh artifact identities

```text
10261b95c7a9d8185d01aab4efabc15803f828fdc2a8c69792a5065637874b97 native_batch_auction.wasm
659545771f26c7b28f2a8bd32e9a19c3e821d93c0f1d9fc624765853b80ee9d5 auction_fault.wasm
f1cab587ee0f98411ae7932f3532d7add81ec9a6a9ece561f88063fbc7af294e native_fulfill_example.wasm
180dc4090f8c1617bc005dbb30fee7dca5cf276a0b9a123c9b1ffb91a769b1b8 native_service.wasm
0248c22cf44f5169d1dee17d2d91a1b4b0f465cd97648b4517d3720934390cf4 native_fault.wasm
a81ee07229ea87aab14ca30457572de963842fcd48a34cf8092252d395b2994a native_dca.wasm
440a1a96468460331f7ef8b4407e395289358199cf2a8bcb660d1f45503189dc mock_exchange.wasm
618fbfa58097aaffe26b5c5197d09207a842b99ef9e1c715229bd02df3fd8cf5 dca_fault.wasm
```

## Whole-branch review and deferred dispositions

The independent whole-branch review covered runtime exclusion/wait admission,
custody/rounding/authority, nested rollback, retained-state restoration, runnable
submission safety and source/model correspondence. It found no new runtime or
custody correctness blocker. It required removal of the tracked Task 6/7
controller reports from the publication set, including reachable history; local
copies are preserved and no public Markdown links depend on them. Its minor
instrumentation naming finding is addressed by `Trace.distinct_record_keys` and
`one_phase_and_at_most_b_distinct_record_keys_per_poll`. A set counts distinct
bid/tick keys even when validation rereads a key, so this test does not establish
an exact storage-read bound. Plan/spec/receipt statuses now reflect completed
Tasks 1–9 and the observed broad review.

Deferred dispositions remain explicit:

- Upstream genesis test-account/balance JSON is noisy but nonblocking. Reducing
  that noise is follow-up work; global stdout and failure diagnostics remain.
- Pinned Quint/npm warnings are development-tool debt under the precise limits
  above. No dependency upgrades or script approvals are included.
- The artifact-writer overlap disclosure and later stable five-case acceptance
  run are proportionate evidence. Builders and artifact readers must be
  serialized; no atomic-builder claim is made.
- The absence of a new correctness blocker is not production-readiness proof
  or merge authorization. Stage A alone is implemented; jobs, waitsets and IBC
  remain on the approved roadmap.

The scoped independent cleanup re-review approved all three findings: publication
tree/history exclusion with local evidence retained, accurate distinct-record
instrumentation, and completion-status reconciliation. It found no new breakage
or new out-of-scope issue. The reviewer inspected the recorded verification and
freshly checked history/tree/source boundaries without rerunning suites. This
completion record closes the administrative gate; it grants no merge or
publication authority.

## Cleanup verification

The amended test-instrumentation tree passed the following fresh commands, each
exit 0, using `CARGO_TARGET_DIR=/private/tmp/auction-rust.vYkeFa`:

```sh
cargo test --workspace --all-features --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
git diff --check
```

Rust again passed 269 tests across 43 result groups, zero failures/ignored,
including the renamed distinct-key test. Full logs are
`/private/tmp/auction-final-cleanup-rust-{test,fmt,clippy}.log`.
The controller separately refreshed ordinary pinned Go (51.779s), deterministic
Quint (129 cases) and its 12 runner tests, six Node commitment vectors, and auction
and legacy DCA shell safety tests; all exited 0. Go and Quint logs are
`/private/tmp/auction-final-cleanup-{go,quint}.log`; Node/shell stdout was inspected
directly. Shell stubs do not constitute a fresh actual-node scenario run.
No expensive unchanged node scenario or randomized/extended Quint run was
repeated during this cleanup. Production Rust, Go and formal sources remain
byte-identical to cleanup base `38a8aeb...`, so their earlier executable evidence,
artifact identities and stated limits above are retained. No concurrent artifact
builder ran during cleanup verification.
