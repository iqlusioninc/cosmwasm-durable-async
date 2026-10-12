# Pinned wasmd native async adapter

This is an experimental real-VM adapter, not a production activation. It links
wasmd **v0.55.1**, Cosmos SDK **v0.50.12**, wasmvm/v2 **v2.2.4**, and Go **1.23.6**.
The Go module checksums are committed. Go 1.27.2 compiles the packages but cannot
link this dependency graph (`encoding/json.unquoteBytes` in upstream sonic), so
the scripts explicitly select 1.23.6.

Build all three real Rust artifacts using `contracts/native-fulfill/build-wasmd.sh`
from the repository root, then run:

```sh
bash integration/wasmd/test.sh
```

`test.sh` delegates to `test-upstream.sh`: both now build the required patched
native runtime and run keeper plus upstream-app tests. Requirements include
`rustc 1.99.0 (b940084d7 2026-09-28)`, Cargo, a C linker, curl, tar and ripgrep.
The verified native build platform is macOS arm64; Linux naming is wired but
has not been exercised by this acceptance run.

The scripts create temporary caches/checkouts. Reuse known complete caches with
`NATIVE_ASYNC_GOPATH`, `NATIVE_ASYNC_GOCACHE`, and `NATIVE_ASYNC_CARGO_HOME`. Do not reuse the old
`/private/tmp/durable-go` cache: its module directories were incomplete skeletons
that Go incorrectly treated as already downloaded. The upstream script copies a
fresh, checksum-verified module checkout, applies exact-anchor transformations,
compiles the actual app, creates a temporary Go workspace, and runs the app tests.
It does not modify upstream module caches or vendor third-party source here.

## Required native error boundary

Stock wasmvm erases backend/cache error provenance into the same string channel
as a Wasm trap. It cannot safely implement the scheduler's infrastructure-failure
policy. `build-vm.sh` copies pinned wasmvm v2.2.4 and downloads CosmWasm v2.2.2
(archive SHA256 `54a75734ef751f76381f08fd595600f4321e6a480b69bf3ceb7debb76dc9f9e9`).
`cmd/patch-vm-boundary` applies exact-anchor source changes and rebuilds the native
library. The bootstrap prints its actual SHA256; binaries are not committed.
It verifies the original native Cargo.lock SHA256
`d185ec9e660b9a607cecbaa69e3adc17d05ae7d60a24bccea75ca2e2210198af`
and changes only six git-source entries to local paths plus lock format v3→v4.
The resulting SHA256 is
`f0562bb3bc97c169f03c8c7c719c5291eb49147432148b0c17caeb73af0c6624`;
all builds use `--locked`, preserving transitive versions/checksums. Cold runs
download Rust/Go dependencies and compile the native library and source tests;
allow several minutes and several GB of temporary build/cache storage.

The extension preserves typed `VmError` through Wasmer user errors. Only real
Wasm trap codes and enumerated guest validation/communication/response errors
become errno 3 (`GuestError`); gas depletion retains errno 2. Backend, cache,
integrity, instantiation, compiler and unknown failures retain errno 1 and become
an `InfrastructureError` panic before wasmd can stringify them. Guest-originated
oversized storage and invalid query JSON get explicit guest variants at their
source, not text-based classification. The app rethrows typed infrastructure
faults before BaseApp's ordinary transaction recovery, and the smart-query RPC
recovery rethrows non-OOG faults. Guest trap/OOG remains a charged task pause.

The additive `native_async_error_abi()` symbol and initialization check fail closed
when patched Go is linked to a stock/stale library. Bootstrap tests that negative
combination before installing the rebuilt library. Direct stock `go test` is now
intentionally unsupported: reuse the printed Go workspace, which includes both
patched wasmd and wasmvm, or rerun the bootstrap. This is an experimental pinned
runtime extension, not an upstream-compatible production activation.

## Integration seams

`Keeper.Options()` decorates the existing messenger, custom query plugin, and
real VM. Ownership comes from wasmd's executing contract address. Registration
and funding transfer actual bank balances to the `native_async` module account;
JSON amounts alone create no credit. Actor/contributor/resolver fields remain
assertions by the owner contract, which must faithfully forward its sender.

The engine guard rejects outer sudo messages before wasmd can dispatch them.
The poll response's encoded messages execute only after the wait is staged in
the same child cache. Immediate callbacks can fill the inbox but cannot poll
recursively. Control changes to the currently executing task are rejected;
topups remain allowed, preserve reservation accounting, and roll back with failed
outbound execution. `Sudo`
supplies real VM/host gas conversion; no second VM conversion or fake gas model
is used. Failure recording and fee settlement survive discarded guest writes.

The application patch creates a dedicated store and module account, installs
options before keeper construction, injects the keeper migration guard after
the ordinary contract/admin checks, installs proposal wrappers, and invokes the
scheduler at the explicitly ordered beginning of EndBlock.

The proposal wrappers independently decode and sum declared transaction gas,
reject overflow/unsupported transactions, trim PrepareProposal, and reject
ProcessProposal above `consensus maximum - async allocation - module EndBlock
allocation`. They pass copied
consensus parameters to the SDK handlers. This is necessary because BaseApp
resets its gas meter after BeginBlock, and SDK no-op mempool handlers otherwise
skip gas checking. `ReserveOrdinaryGas` is only a keeper-test helper.

`ApplicationEndBlock` runs the scheduler and ordinary modules in one cache. Modules
receive a fresh finite `ModuleEndBlockGas` meter, even if async maintenance used
its entire separate slice. Actual module gas is added to the full shared block
meter; module errors/OOG discard the entire EndBlock cache and propagate to ABCI.
Invalid allocation arithmetic is rejected at app construction, genesis and
proposal processing. The explicit accounting claim covers ordinary transactions,
async work and EndBlock modules. BeginBlock retains upstream SDK accounting and
its subsequent gas-meter reset; these changes do not claim to meter all existing
BeginBlock work against this partition.

## State and reference-model mapping

- Global monotonic task ID, per-contract readiness index, persistent contract
  rotation, timer/expiry/prune indexes; each queue/maintenance visit is bounded.
- Registration and scheduler-generated wakes are H+1; ordinary callbacks may
  wake at H. Deadlines are checked directly, and accepted inboxes survive their
  deadline. Task expiry dominates all execution and callbacks.
- Success commits continuation, inbox consumption, Wasm writes and outbound
  messages together. Guest errors, OOG, invalid responses and message errors roll
  back the child cache, preserve old state/inbox, charge real gas, and pause.
- Prepaid allocated-byte/finite-lifetime rent and cleanup fees; remaining
  execution escrow is refunded on terminal transitions. Terminal records retain
  their byte allocation until the original lease endpoint and are then pruned.
- Metadata slack, bounded errors, U128 balance limits, and an admission bound on
  failure-recording gas prevent user-selected continuation sizes from removing
  the mandatory accounting allowance. Maintenance OOG discards only its current
  visit. Empty contract queue entries are removed promptly.
- All shared JSON fixtures decode into concrete Go production ABI types.
- Public Task queries project the shared Rust `TaskResponse` DTO, including
  structured paused status and explicit nullable fields. Internal queue keys,
  accounting fields and Go persistence records are not exposed through that ABI.

The Rust model and Go keeper are separate implementations. The Go integration
currently measures gas with real SDK/wasmvm meters; the Rust reference accepts
executor gas results. Both interpret `gas_per_poll` as the total SDK cap: the
guest slice is `cap - HostReserve`, and admission/funding reserve the total cap
once. Both bound Success by decoded data bytes and Error by UTF-8 code plus
message bytes. Inbox storage separately reserves `6 * max_result_bytes + 128`
for worst-case JSON escaping. `testdata/result-boundaries.json` is shared with
the Rust boundary test. Go additionally reserves task metadata and indexes.

## Evidence and remaining acceptance work

The default suite executes the generated Rust contract and two real Wasm service
instances: registration, automatic next-block start, two authenticated callbacks,
completion, and refund. It also executes a real outbound-message failure and
checks rollback, charged fees and durable keeper reconstruction. Other tests
cover typed ABI fixtures, proposal partition/overflow with no-op handlers,
maintenance OOG atomicity, persistent round-robin ordering, in-flight control
rejection, and topup conservation on successful/failed dispatch. Canonical pause
reasons are separate from bounded diagnostic messages. The upstream-tagged suite
checks actual app construction, EndBlock invocation, module meter isolation and
the patched real keeper migration rejection.

Additional acceptance uses signed ABCI transactions, disk close/reopen and
deterministic replay. Real callback storage panics through Sudo, nested Execute,
Query and Reply propagate without task charges or committed child writes; an
ordinary signed transaction proves BaseApp does not turn a typed native fault
into a transaction error code. Snapshot and network state-sync coverage is
described below; this is not a multi-validator deployment test. Outbound reply/data aggregation and IBC-specific resolver
adapters are not provided.

ABI1 rejects Wasm start sections deterministically at validation and raw-Wasm
compilation, including unchecked imports/cache rebuild. Wasm start imports run
before upstream initializes its host/gas context, so accepting them could turn
guest-controlled code into a native lifecycle panic. Contracts using a Wasm
start section must be rebuilt without one. The native cache discriminator is
changed to prevent reusing pre-extension compiled modules. Structured fallback
classification preserves `InstantiationError::Start`/link traps, treats guest
import type/missing-import errors as validation failures, and leaves resource,
CPU-feature, architecture and store mismatches as infrastructure failures.

Config is deterministic constructor data, not mutable governance state.
`ExperimentalConfig` is a local-harness policy with explicit uncalibrated prices;
the default keeper tests use zero storage rent plus a cleanup fee to isolate
execution accounting, while the patched app uses nonzero storage rent. Final
bank transfer/accounting writes are metered in the reserved host slice and
covered by prepaid maintenance fees; the execution fee is settled from the
attempt plus its first host disposition write. Lease growth, budget changes and
refund-address management have parity tests. General privileged import validation,
exhaustive failure and overload coverage, and production calibration remain follow-ups.

## State sync

The mounted native scheduler store travels in the SDK's authenticated IAVL
snapshot alongside bank and contract state. A genesis-only manifest commits the
native schema and scheduler configuration. Restore and ordinary startup reject
missing or incompatible manifests rather than inventing them for existing data.
This changes genesis state: pre-manifest experimental databases need an explicit
upgrade, or a fresh experimental chain; they are not silently migrated.

The Wasm snapshot extension verifies bounded decompressed Wasm size, code checksums,
membership, uniqueness and completeness before activation. Raw Wasm is restored
and compiled locally; machine-specific compiled caches are not transferred.
The final chunk must match the offered trusted app hash, and read-only checks
validate the scheduler's task/index/counter and escrow invariants. Omitting the
entire Wasm extension does not bypass the final checks. Restore never polls a
task, advances its continuation, or charges task escrow.

Before accepting restoration, the app durably records
`HOME/.native-async-restore.pending` (`HOME` here means the node's configured app
home, not the user's shell home). It clears that marker only after successful
verification. Partial or failed restoration blocks execution in-process and
after a process restart. The pinned SDK cannot reset partially imported IAVL
state for another attempt: use a fresh node data directory after failure; do
not bypass the guard by deleting the marker and reusing rejected data.

`TestSnapshot*` uses actual ABCI snapshot offer/load/apply calls and empty LevelDB
and Wasm directories. It covers pinned/unpinned code, runnable and waiting tasks,
paused tasks with accepted inboxes, terminal records, deadline/expiry crossings,
duplicate callbacks, and subsequent identical roots/events/gas/balances. Negative
cases rebuild streams with valid chunk metadata but missing, unexpected,
duplicate or oversized code, omitted extensions, incompatible configuration or
the wrong trusted hash. Ordinary corrupted chunks exercise refetch separately.
Successful restoration also survives reopening the app; incomplete and rejected
restores refuse reopening instead of forgetting the activation guard.

The opt-in network test starts three independent localhost processes: one
validator, a second snapshot-serving full node, and an empty state-sync joiner.
CometBFT verifies the trusted height/hash and fetches the snapshot from peers.
The test checks that the joiner did not replay from genesis, preserves task and
funding state, completes the workflow after a callback, and agrees on subsequent
app hashes and historical-height bank balances. Run the complete build and test
route with this additional test enabled:

```sh
NATIVE_ASYNC_NETWORK_TEST=1 bash integration/wasmd/test.sh
```

To reuse a previously printed patched workspace instead of rebuilding the native
library, run from `integration/wasmd` with that `GOWORK` and the documented Go
cache/toolchain environment:

```sh
NATIVE_ASYNC_NETWORK_TEST=1 go test -tags upstream -count=1 -timeout 5m -run 'TestSnapshot|TestStateSyncNetwork' ./...
```

These are local pinned-version acceptance tests, not proof of multi-validator
consensus under faults, Byzantine-peer resilience, cross-platform determinism,
general genesis export/import, or production resource calibration.
