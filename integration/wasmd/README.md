# Pinned wasmd native async adapter

This is an experimental real-VM adapter, not a production activation. It links
wasmd **v0.55.1**, Cosmos SDK **v0.50.12**, wasmvm/v2 **v2.2.4**, and Go **1.23.6**.
The Go module checksums are committed. Go 1.27.2 compiles the packages but cannot
link this dependency graph (`encoding/json.unquoteBytes` in upstream sonic), so
the scripts explicitly select 1.23.6.

Build both real Rust artifacts using `contracts/native-fulfill/build-wasmd.sh`
from the repository root, then run:

```sh
bash integration/wasmd/test.sh
bash integration/wasmd/test-upstream.sh
```

The scripts create temporary caches/checkouts. Reuse known complete caches with
`NATIVE_ASYNC_GOPATH` and `NATIVE_ASYNC_GOCACHE`. Do not reuse the old
`/private/tmp/durable-go` cache: its module directories were incomplete skeletons
that Go incorrectly treated as already downloaded. The upstream script copies a
fresh, checksum-verified module checkout, applies exact-anchor transformations,
compiles the actual app, creates a temporary Go workspace, and runs the app tests.
It does not modify upstream module caches or vendor third-party source here.

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
  messages together. VM errors, OOG, invalid responses and message errors roll
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

The keeper reconstruction test is not node restart, replay, state-sync or a
multi-validator app-hash test. The initial app test directly calls EndBlock; full
ABCI FinalizeBlock/proposal acceptance remains separately tracked acceptance
work. Outbound reply/data aggregation and IBC-specific resolver adapters
are not provided.

Config is deterministic constructor data, not mutable governance state.
`ExperimentalConfig` is a local-harness policy with explicit uncalibrated prices;
the default keeper tests use zero storage rent plus a cleanup fee to isolate
execution accounting, while the patched app uses nonzero storage rent. Final
bank transfer/accounting writes are metered in the reserved host slice and
covered by prepaid maintenance fees; the execution fee is settled from the
attempt plus its first host disposition write. Lease growth, budget changes,
refund-address changes, privileged import validation, exhaustive failure
and overload coverage, and production calibration remain follow-ups.
