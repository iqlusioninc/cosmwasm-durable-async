# Local native DCA tutorial

This example creates a finite order that swaps native test coins over several
blocks using a generated durable workflow. It uses the
[mock exchange](../mock-exchange/README.md), with a fixed integer price and local
output liquidity. It is an experimental developer example, not a financial
product, live market, oracle or production deployment.

## Run from the repository root

Install rustup, Cargo, Go, a C linker, curl, tar and ripgrep. The repository pins
Rust `1.99.0 (b940084d7 2026-09-28)` and its Wasm target; the bootstrap selects
Go `1.23.6`. macOS arm64 is the verified native build platform. On that platform
the artifact builder downloads checksum-verified Binaryen 123 if `WASM_OPT` is
unset. On other platforms, supply `WASM_OPT` pointing to Binaryen 123; Linux
native-library names are wired but this tutorial's acceptance does not establish
Linux support.

```sh
bash scripts/native-dca-demo.sh happy
bash scripts/native-dca-demo.sh funding
bash scripts/native-dca-demo.sh liquidity
bash scripts/native-dca-demo.sh cancel
bash scripts/native-dca-demo.sh restart
# Run every scenario:
bash scripts/native-dca-demo.sh all
```

The runner uses the opt-in `TestNativeDCADemo` in the pinned Go harness and one
owned local child validator. Stock `wasmd` is insufficient: the contracts need
the native custom message/query routes, privileged poll scheduler, escrow and
patched wasmvm error boundary. The runner provisions deterministic test
identities and genesis assets, accepts only loopback RPC, and cleans up its own
child and temporary home. Never use its identities or coins outside this test.

The script prints a retained output directory. Each scenario creates a unique
subdirectory containing `receipt.json`, `node.log`, and `tx-NNN.json` journals.
Set `NATIVE_DCA_OUTPUT` to an existing directory to choose their destination.
Receipts contain transaction hashes/outcomes, committed state heights/app hashes,
order/task/funding snapshots, fills, bank balances and owned-child exit status.
Chain homes and validator keys are temporary; wallet signing keys stay in memory.
After the test exits its RPC endpoint is closed. Inspect the saved evidence, for
example with optional `jq`:

```sh
demo_receipts="$(mktemp -d)"
NATIVE_DCA_OUTPUT="$demo_receipts" bash scripts/native-dca-demo.sh happy
jq '.snapshots[] | {label, committed, order: .state.Order, funding: .state.Funding}' \
  "$demo_receipts"/happy-*/receipt.json
```

Cold setup downloads pinned Go modules, Rust dependencies, CosmWasm v2.2.2 and
the Binaryen archive where applicable; it compiles the patched VM and wasmd app.
Allow several minutes and several GB of temporary storage. See
[bootstrap and reuse](../../integration/wasmd/README.md#local-dca-scenarios) for
the supported environment variables and targeted reruns. To build only the
normalized contract artifacts:

```sh
bash contracts/native-dca/build-wasmd.sh
```

This writes `artifacts/native_dca.wasm`, `artifacts/mock_exchange.wasm` and the
acceptance-only `artifacts/dca_fault.wasm`, and prints their SHA256 hashes. The
builder respects `CARGO_TARGET_DIR` and `WASM_OPT`. The fault contract is not the
exchange used for the runnable scenarios.

## What each scenario demonstrates

| Command | State to inspect |
| --- | --- |
| `happy` | Three committed fills, beneficiary output, remaining principal, then separate host completion and gas refund |
| `funding` | Pause before a trade, sponsor gas top-up without resumption or new authority, explicit owner resume |
| `liquidity` | Failed swap rolls back business effects while charging gas; replenish output liquidity, then owner resume |
| `cancel` | Owner cancellation and explicit reclaim of only unspent principal; an earlier fill stays paid |
| `restart` | Stop the child after a committed fill, reopen the same home, then consume the saved receipt without another trade |

Blocks advance through test-only deterministic permits. The restart scenario
persists a gate before the next FinalizeBlock after the selected fill commit,
so it observes the fill-before-consumption window directly. It is a real single
local child process restart, not evidence of free-running execution cadence,
throughput, multi-validator operation or DCA state sync.

Comet may replay an already decided block during startup before opening RPC.
The restarted child therefore records fresh committed-app queries before that
handshake; the parent checks the owned child's PID, height, app hash, complete
order/task/funding/fills and bank balances before releasing the persisted gate.
Before handshake, the query context uses the exact committed height/time header
saved after Commit, bound to the reopened app hash; missing or mismatched
metadata fails closed. This is test observation, not runtime state reconstruction.
That snapshot is labeled `child_startup_committed_queries`, not RPC evidence.
Normal handshake replay then proceeds, and live RPC confirms receipt consumption
without another trade. All other scenario snapshots are labeled `rpc`.

Each transaction is journaled with exact signed bytes and hash before submission.
An uncertain RPC outcome is looked up by that same hash; the runner stops if it
cannot resolve it and never broadcasts an attempted journal again. Do not infer
failure from a missing RPC response or manually resubmit an ambiguous trade.

## Orders, funding and queries

Instantiation fixes `exchange` and `fee_denom`. `create_order` supplies
`input_per_slice`, `slice_count` (1–128), positive `minimum_output`, positive
`interval_blocks`, `beneficiary`, initial `gas`, `gas_per_poll`,
`max_gas_price`, immutable `expires_at` and `allocated_bytes`. The actual sender
becomes owner. Attached coins must exactly cover principal plus initial gas;
when those denominations coincide, their amounts are coalesced. Unexpected,
duplicate, zero or excess coin entries are rejected. Registration returns both
the contract-local `order_id` and host `task_id`.

Principal remains in the DCA contract; only the specified gas allocation enters
host escrow. In the local harness, principal is `uinput`, beneficiary output is
`uout`, scheduler escrow is `ugas`, and ordinary signed transactions pay `stake`.
These are separate balances. In particular, a plain bank transfer to the DCA
contract is neither order principal nor task funding.

Use these smart-query payloads with the order/task IDs reported by the runner
(the numeric IDs below are illustrative):

```json
{"order":{"order_id":1}}
{"fill":{"order_id":1,"slice_index":0}}
{"task":{"order_id":1}}
{"async":{"funding_config":{}}}
{"async":{"task_funding":{"task_id":1}}}
```

`order` returns stored terms, `filled_count`, `spent`, `received`, `remaining`,
`reclaimed`, `last_fill_height` and separate host information. `fill` returns the
receipt and committed height. `task` reports host progress. A pruned/unavailable
host record is explicit, not a successful-completion claim. Always check both
business fills and host status: a trade pays the beneficiary and records its
fill in one transaction, while the host consumes its successful inbox no earlier
than the next block. Even all slices filled does not mean the workflow has
already completed.

Illustrative state summary, not literal runner output or a verification receipt:

```text
order_id=1 task_id=1 filled_count=1/3 spent=10 remaining=20 reclaimed=0
host=runnable (the committed fill receipt is awaiting consumption)
```

This state has one paid trade even though the host can still run. Compare the
fill receipt, beneficiary bank balance and funding query before advancing.

Inspect funding independently for available escrow, cumulative fees and current
maximum-attempt charge. The RPC demo uses authoritative funding/bank queries:
the current public ABCI response omits scheduler `async_poll` events. Quoted
execution prices and measured gas in this harness are uncalibrated test values.

## Timing and recovery rules

The first eligible target is registration height plus `interval_blocks`. Every
later target is the previous **actual committed fill height** plus that interval.
Targets are minimum eligible heights, not promised execution times or seconds.
For example, with interval 3, a target at height 10 filled late at 12 makes the
next target 15. There is one slice per dispatch and no missed-interval catch-up
batch. A timer or operation deadline must be strictly before immutable expiry;
if the next wait cannot fit, the workflow ends with `LeaseWindowExhausted` and
leaves unspent principal for explicit reclaim.

Order terms, expiry, beneficiary and exchange cannot be edited. The owner can
update `gas_per_poll`/`max_gas_price` with `update_budget`, but that does not
resume a paused task. Sponsors may submit `async.fund_task` with the fee coin;
they gain no owner control or refund rights, and top-up does not resume. The
owner must submit `async.resume_task` after correcting a recoverable condition.
The corresponding execute payloads use the host task ID for controls and the
contract order ID for reclaim (IDs below are illustrative):

```json
{"async":{"fund_task":{"task_id":1}}}
{"async":{"resume_task":{"task_id":1}}}
{"async":{"cancel_task":{"task_id":1}}}
{"reclaim_principal":{"order_id":1}}
```

Attach the fee coin only to `fund_task`; resume, cancel and reclaim accept no
funds. Use the owner for control/reclaim transactions, and a sponsor or owner
for funding.

Insufficient liquidity can be corrected by sending output test coins to the
exchange. An impossible minimum output cannot be corrected by changing the
fixture's price or editing the order: cancel and reclaim instead.

Cancellation uses `async.cancel_task`; reclaim is a separate owner-only
`reclaim_principal` execute. Reclaim is available after host termination or at
the immutable expiry boundary and pays only that order's remaining principal.
It cannot pay twice, undo a committed trade or reclaim gas escrow. Host terminal
gas refunds go separately to the owner. After expiry, principal recovery does
not depend on the host retaining its task record. The accounting invariant is:

```text
initial principal = spent + remaining + reclaimed
```

A failed nested swap/callback/reply transaction rolls back its business writes
and transfers while measured poll gas remains charged. An unexpected timeout or
remote error terminates diagnostically without a replacement swap; there is no
automatic timeout retry. Cancellation and expiry never reverse an earlier fill.

## Evidence boundaries

The [approved design](../../docs/superpowers/specs/2026-10-15-native-async-examples-design.md)
defines the example. Transactional Rust tests and pinned real-VM acceptance
separately cover rollback, authority, accounting and database close/reopen.
Injected engine boundary failures are test evidence for those injected paths;
database reopen is distinct from the runner's child process restart. Neither
establishes a new formal proof, DCA state-sync proof or production calibration.

Known runtime issues N1 (Rust zero-price in-flight guard), N2 (Rust/Go wait
acceptance mismatch) and R1 (visit-budget starvation policy) remain open. This
demo uses validated addresses, a strictly positive minimum base price and the
stricter due-before-expiry policy. It makes no general fairness or runtime-repair
claim. Escrow and auction examples remain separate follow-up work.

The [verification receipt](../../docs/reviews/2026-10-15-native-dca-verification.md)
records source/artifact identities, commands, measured scenario evidence and
remaining review boundaries.
