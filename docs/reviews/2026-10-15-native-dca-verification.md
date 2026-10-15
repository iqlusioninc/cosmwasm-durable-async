# Native DCA implementation verification — 2026-10-15

Implementation and the commands below pass in the isolated macOS arm64 worktree.
Tasks 1–6 have independent task reviews. The whole-change review reported no
Critical/Important findings and two scoped runner/test fixes. **Those fixes await
controller re-review; Task 7 and final approval remain pending.** This receipt is not review approval,
a production-readiness claim, or permission to publish/deploy.

## Source and artifact identity

Implementation commits before the tutorial:

| Task | Commit |
| --- | --- |
| Protocol | `430998bc1ed899938c495f19e97b008bb8e95f0a` |
| Fixed-price exchange | `a55635f969cdff2c9d8b64a266cd070ad20a375e` |
| Transactional reference adapter | `5d8ad05509f87853669bedf0dbd48acaa5114aca` |
| Admission/custody | `f4f4b5b21ae5b62a698e095b3c01378e15ba163f` |
| Workflow/settlement | `614b917ee3a1f60d22c3ad95d182a2ece1cc9752` |
| Pinned Wasm acceptance | `3aec95b0af0fabefd13be70f31d2b9c24edd5bd8` |

The Task 7 commit containing this receipt adds the runner, journals and tutorial;
its tested executable sources are identified independently below. Resolve that
commit with `git log -1 --format=%H -- integration/wasmd/dca_demo_node_test.go`.

| Tested source | SHA256 |
| --- | --- |
| `integration/wasmd/dca_demo_test.go` | `1b65456b1e0fba43e9105ab2f39e134edbd620aad6ece65d7301dd834ea9e967` |
| `integration/wasmd/dca_demo_node_test.go` | `958e6890ae3be78778192577badf70dc07f2b72f0596f996de5c5387f05eacea` |
| `integration/wasmd/dca_demo_journal_test.go` | `27897c17da9c01f7893ba58e68dd20659fdb0d4e09b9fef14d4bdf904aff3476` |
| `integration/wasmd/dca_acceptance_test.go` | `3891836cccb3487a1ebe8d5da295d029a3aca2e77113d56f33047e49ec40b725` |
| `scripts/native-dca-demo.sh` | `a46a646bb1d56b17bf24820aee63e5db9921224b033c006820f4663918fd0a11` |
| `scripts/native-dca-demo-test.sh` | `529b1b480d6952b8ea3c14205aa4a1f29ce1cd6a19a5bde2e20a0e872c3ee4d4` |
| `integration/wasmd/test-upstream.sh` | `57d647c7db456723f3817a2c436d533acaac2a1ab5ac6e61c31015fbc27bd384` |

Both builders were rerun after implementation. All artifact hashes match Task 6:

| Artifact | SHA256 |
| --- | --- |
| `native_dca.wasm` | `a81ee07229ea87aab14ca30457572de963842fcd48a34cf8092252d395b2994a` |
| `mock_exchange.wasm` | `440a1a96468460331f7ef8b4407e395289358199cf2a8bcb660d1f45503189dc` |
| `dca_fault.wasm` | `618fbfa58097aaffe26b5c5197d09207a842b99ef9e1c715229bd02df3fd8cf5` |
| `native_fulfill_example.wasm` | `f1cab587ee0f98411ae7932f3532d7add81ec9a6a9ece561f88063fbc7af294e` |
| `native_service.wasm` | `180dc4090f8c1617bc005dbb30fee7dca5cf276a0b9a123c9b1ffb91a769b1b8` |
| `native_fault.wasm` | `0248c22cf44f5169d1dee17d2d91a1b4b0f465cd97648b4517d3720934390cf4` |

Toolchain: Rust/Cargo 1.99.0, Go 1.23.6, Binaryen 123, wasmd 0.55.1,
Cosmos SDK 0.50.12, wasmvm 2.2.4. The already fresh, controller-verified pinned
bootstrap was reused rather than rebuilt redundantly:

```sh
export CARGO_TARGET_DIR=/private/tmp/native-dca-target.BHoTPC
export WASM_OPT=/private/tmp/binaryen-version_123/bin/wasm-opt
export GOTOOLCHAIN=go1.23.6
export GOPATH=/private/tmp/native-dca-go.XoQoxD
export GOCACHE=/private/tmp/native-dca-go.XoQoxD/build
export GOWORK=/private/tmp/native-async-workspace.SzyQDE/go.work
export NATIVE_DCA_OUTPUT=/private/tmp/native-dca-demo-runs.EzW4xq
```

The workspace references patched wasmd `/private/tmp/native-async-wasmd.xajLiQ`
and VM `/private/tmp/native-async-vm.SYgidV/wasmvm`; its native library SHA256 is
`51a5a64866742f58167ca2b40066ecdb1b83f49fee02583f1ae255b8f6c44a7b`.
These temporary paths identify this run, not portable prerequisites; follow the
[tutorial](../../contracts/native-dca/README.md) for a fresh setup.

## Commands and observed results

From the repository root, in the environment above:

```sh
cargo test --workspace --all-features --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
bash contracts/native-fulfill/build-wasmd.sh
bash contracts/native-dca/build-wasmd.sh
bash -n scripts/native-dca-demo.sh scripts/wasmd-normalize.sh \
  contracts/native-dca/build-wasmd.sh contracts/native-fulfill/build-wasmd.sh \
  integration/wasmd/test-upstream.sh
git diff --check
```

All pass. Rust: **205 tests, 33 result groups**, no failed/ignored tests. Strict
clippy and final Rust logs have no warning/error diagnostics. DCA artifact
builds also ran inside every scenario script; the earlier permission-only
attempt could not write the managed worktree artifact, and the authorized
scoped retry passed. That environment failure is not behavioral RED evidence.

From `integration/wasmd`, with the same Go workspace/cache:

```sh
go test -tags upstream -count=1 -run '^TestDCADemo(Journal|RPC|Cleanup)' .
go test -tags upstream -count=1 ./...
NATIVE_DCA_DEMO=1 NATIVE_DCA_SCENARIO=restart \
  go test -race -tags upstream -count=1 -v -timeout 3m \
  -run '^TestNativeDCADemo$|^TestDCADemo(Journal|RPC|Cleanup)' .
```

All pass. Full pinned Go suite: **32.381s**, including real-VM DCA acceptance
and existing regressions; the ordinary suite explicitly skips opt-in node demos.
Focused race run: **16.186s**, including the actual child restart and all ten
journal/RPC/cleanup cases; its helper-only test skips in the parent by design.

Each root-level command below was run independently and passed; `all` passed
again against the final runner (**38.873s**, five scenario subtests):

```sh
bash scripts/native-dca-demo.sh happy
bash scripts/native-dca-demo.sh funding
bash scripts/native-dca-demo.sh liquidity
bash scripts/native-dca-demo.sh cancel
bash scripts/native-dca-demo.sh restart
bash scripts/native-dca-demo.sh all
```

Actual retained receipts, with `passed=true` and `owned_child_exited=true`:

| Scenario | Receipt directory under `NATIVE_DCA_OUTPUT` | Final state |
| --- | --- | --- |
| happy | `happy-1116097938` | H23, 3 fills, host completed, gas escrow zero |
| funding | `funding-2781654520` | H28, 3 fills after sponsor top-up and explicit owner resume |
| liquidity | `liquidity-1474876085` | H26, 3 fills after liquidity replenishment and owner resume |
| cancel | `cancel-1737296927` | H25, cancelled, 1 paid fill, 20 input explicitly reclaimed |
| restart, final all run | `restart-3294134303` | H23, 3 fills, one process restart, host completed |
| restart, race run | `restart-2037806522` | Same outcomes, race detector clean |

Each directory contains `receipt.json`, node log and exact signed transaction
journals. Per-run logs are `/private/tmp/native-dca-go.XoQoxD/task7-*.log`;
Rust logs are `task7-workspace-{test,clippy}.log` under the selected Cargo target.
The runner removes its owned chain homes/test validator keys through Go test
temporary-directory cleanup, and reaps every owned child. It retains only
small receipts/logs/journals, not additional chain databases or build trees.
The cleanup regression separately confirms an unrelated child remains alive.

## Journal and restart failure evidence

### Final-review fix checks

The existing output directory is now canonicalized before export and the later
runner `cd`. `bash scripts/native-dca-demo-test.sh` first failed against the
original runner (`wrong output directory after cd: output with spaces`), then
passed after the fix. It executes the actual runner with builder/Go boundaries
stubbed, verifies the existing relative path containing spaces and receipt
creation after the working-directory change, and starts no VM/validator.

Funding, liquidity and restart now require final business count 3, remaining
principal `0`, and task output `"Filled"`, independently of host completion.
Test-only mutations of count to 2, principal to `10`, and output to
`"LeaseWindowExhausted"` each failed at its new assertion (three failing
subtests, 28.526s). All mutations were removed; no runtime/contract changes.

With the prepared environment above and
`NATIVE_DCA_OUTPUT=/private/tmp/native-dca-final.IVbzgG`, fresh checks pass:

```sh
bash scripts/native-dca-demo-test.sh
bash -n scripts/native-dca-demo.sh scripts/native-dca-demo-test.sh \
  scripts/wasmd-normalize.sh contracts/native-dca/build-wasmd.sh
gofmt -l integration/wasmd/dca_demo_test.go
bash scripts/native-dca-demo.sh all
git diff --check
```

The final script all-run passes **40.907s**, all five subtests. DCA artifact
hashes remain unchanged. Logs `fix-script-{red,green}.log`,
`fix-assertion-mutation-red.log`, `fix-all-preliminary.log` (PASS40.527s), and
`fix-script-all-green.log` are retained under that output directory. Final
successful receipt directories are `happy-218204741`, `funding-723154199`,
`liquidity-3086858592`, `cancel-1049176635`, `restart-2815838791`, all with
`passed=true` and `owned_child_exited=true`. Failed mutation receipts remain
failed evidence. These fixes await controller scoped re-review/final approval.

The journal's compilable stubs first produced eight semantic RED failures;
implementation and race checks then passed. Coverage includes lost response,
corrupt bytes/hash, existing intent refusal, concurrent callers, persisted
attempted crash-stage lookup only, definitive CheckTx rejection, bounded RPC and
preserving original intent/result evidence. Owned-child cleanup separately
failed with the no-op cleanup stub, then passed after stop/reap implementation.
The first opt-in scenario stub also failed before the runner existed.

Initial intent installation uses a synced same-directory temporary file and
hard link without replacement; updates use synced temporary files and atomic
rename. Both sync the directory. A stable, never-unlinked advisory lock covers
all mutation and the bounded broadcast/lookup operation. Scope is cooperating
processes on supported local Darwin/Linux filesystems, not a distributed log.
`attempted` is durable before RPC; even a crash before submission is uncertain
and permits only lookup of the original hash. Unknown codes/heights stay null.
No test wallet signing key is persisted in these journals.

Restart initially failed waiting for RPC: Comet's NewNode handshake replays an
already decided next block before `startRPC`, while the persisted fill gate
correctly prevents consumption. A first pre-handshake query attempt exposed
BaseApp's unset check-state block time; real Wasm rejects that empty environment.
Both failed receipts/logs (`restart-1515887205`, `restart-588160491`) remain with
`passed=false` and child-exit evidence. They are not relabeled as success.

The final child queries its reopened committed app before NewNode, using the
exact height/time/chain header saved after the preceding real Commit and bound
to its committed app hash. Missing/mismatched metadata fails closed; normal
post-handshake RPC contexts are unchanged. Startup evidence must be absent
before launch and match the fresh owned PID. The parent compares the complete
order/task/funding/fill/bank/root snapshot before releasing the separate
persisted selected-fill gate. No state field or equality check is relaxed.
Snapshot source is `child_startup_committed_queries`; all other observations
are `rpc`. Normal replay then consumes the inbox exactly once.

The final all-run restart preserved H16 root
`4078A11A54332B1105A9159A748529DA34B161851832253531E595349AD06BD0`,
one fill, spent 10, remaining 20, and the unconsumed Success inbox across the
fresh process. After release at H17, the same one fill remains; later fills are
H19/H22, and host completion is H23. A positive gas fee is charged for receipt
consumption separately from the already committed trade.

## Eight validation groups and evidence boundaries

| Design group | Evidence |
| --- | --- |
| Happy path | Rust workflow tests, `TestDCAAcceptanceAtomicReceipt`, actual-node happy script; exact beneficiary/principal balances and final gas refund |
| Scheduling | Rust `three_slices_have_minimum_interval_and_no_catchup`, strict expiry/overflow and generated allocation tests; Go three-fill heights; runner fills H16/H19/H22 |
| Funding | Rust sponsor/price-pause/cap-change cases, `TestDCAAcceptanceFundingAndCancellation`, RPC funding scenario; top-up never resumes or changes owner/refund |
| Atomicity | Rust 18-row nested fault matrix, Go nine-row failure table and exact 2048/2049 host result boundary; business rollback checked independently of charged gas |
| Authorization/accounting | Admission and staged-identity tests, two owners/contracts, shared gas/principal denomination and repeated reclaim; real-VM balances and owner/sponsor transactions |
| Lifecycle | Rust cancellation/expiry/pruning and diagnostic no-retry cases; Go before/after-fill cancellation, post-receipt expiry/pruning/reclaim; RPC cancellation after one paid fill |
| Persistence | In-memory Rust reconstruction, pinned Go database close/reopen before timer/after receipt, and the distinct actual child-process restart described above |
| Build/VM | Both normalized artifact builders and hashes, real Wasm nested self/exchange/bank/callback/Deliver/reply, pinned SDK gas/funding tests and full regressions |

The synchronous reviewed fixture is the counterparty trust assumption. Boundary
decorators for host delivery/reply/result-size tests execute genuine Wasm and
preserve returned VM gas before controlled injection; these are labeled injected
boundaries, not naturally occurring exploits. The omitted-callback test reaches
the untouched production reply guard. Rust adapter gas is a fixed logical
schedule; real SDK/VM gas belongs to pinned Go acceptance, not that Rust model.

The local node is deliberately gated for inspectable exact heights: normal
Comet consensus, mempool, signed RPC transactions, FinalizeBlock and Commit
still perform every transition. A preassembled empty block is released before
the next block includes a submitted transaction; registration height is checked
against the quote. Per-connection ABCI clients and a test-only app mutex permit
queries while the next FinalizeBlock waits outside that mutex. Waits are
shutdown-aware; RPCs are capped at 2 seconds, journal resolution at 15 seconds,
each scenario at 90 seconds and 60 observed-block steps.

This is not free-running cadence/throughput evidence, multi-validator or DCA
state-sync proof, a new Quint model/proof, production gas calibration, runtime
repair, IBC support or unrestricted Rust-future persistence. N1/N2/R1 remain
open. The demo uses validated addresses, a positive minimum base price and strict
due-before-expiry bounds. Public ABCI currently omits scheduler `async_poll`
events; RPC metrics use authoritative funding/bank state. Acceptance's genuine
pre-Commit context-event observation is separately labeled instrumentation.
Milestone escrow and auction designs remain separate follow-ups.
