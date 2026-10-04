# Compiled-Wasm validation

Run from the repository root:

```sh
scripts/install-binaryen.sh
scripts/check-wasm.sh
```

The installer fetches Binaryen **123** for Linux x86_64/aarch64 or macOS
x86_64/arm64, checks a committed SHA-256, and extracts under ignored
`target/tools`. It does not install a global tool. `WASM_OPT` may instead identify
an existing Binaryen 123 executable.

`check-wasm.sh` builds with the repository's Rust **1.99.0**, normalizes the
artifact, applies the compatibility validator used by **cosmwasm-check 2.2.2**,
and executes it using **cosmwasm-vm 2.2.2 / Wasmer 4.3.7**. The host harness has a
separate committed lockfile at `tools/vm-tests/Cargo.lock`; its large native VM
dependencies are excluded from contract builds. CI performs these checks and
uploads the checked artifact and checksum.

## Artifact compatibility

Use `target/artifacts/fulfill_example.wasm`, not the raw Cargo output, for the
2.2 VM target. `scripts/build-wasm.sh` accepts contract package names, for example:

```sh
scripts/build-wasm.sh fulfill-example
```

Each output has a `.wasm.sha256` sidecar. The build pins the compiler, optimizer,
and dependency lockfile; record this checksum alongside a chain upload. This
is an identity check, not a claim that arbitrary host platforms produce identical
bytes.

A raw Rust 1.99 build succeeds but the 2.2.2 VM rejects its `memory.copy`
instructions. Merely selecting Rust 1.85 or disabling target features also left
precompiled standard-library indirect-call encodings rejected by static
validation. The build pipeline therefore retains the workspace compiler and
runs Binaryen 123 with:

```text
-Oz --enable-bulk-memory --enable-reference-types --llvm-memory-copy-fill-lowering
```

Binaryen canonicalizes encodings and lowers memory copy/fill to operations
accepted by this VM. **The resulting artifact must still pass the validator and
execution tests**; an optimizer invocation alone is not compatibility evidence.
The script deliberately does not change the global Rust toolchain or weaken
VM feature checks.

## What the tests establish

The test harness runs the compiled Wasm, preserving host storage but destroying
and recreating Wasm memory between invocations. It checks:

- instantiate, start, two authenticated callbacks, restored order, terminal
  receipt, emitted messages, and active count;
- unauthorized/stale callbacks leave host storage byte-for-byte unchanged;
- exact deadline rejection and permissionless expiration;
- an application error produces a terminal failure while earlier segment
  writes remain;
- an insufficient execution budget produces `VmError::GasDepletion`;
- payment callback payload costs at 1, 1,024, and 8,192 bytes.

The harness uses the VM's **mock host API, storage, and querier**. Response
messages are inspected but not dispatched. These are separate VM invocations,
not separate blockchain transactions. It provides no SDK cache rollback. The
failed out-of-gas backend is discarded without claiming that the VM restored it.
Use chain tests for message execution, fees, SDK gas conversion and rollback.

`VM_METRIC` reports internal CosmWasm gas, mock-host gas, and the sum of persisted
key/value lengths immediately after payment resumption. This is a **callback
payload sweep**, not a checkpoint-size benchmark: the example checkpoints only
the fixed `Order`. Storage excludes database/IAVL overhead and terminal
retention over many workflows. These numbers are not transaction gas estimates
or performance guarantees.

## Chain target and remaining coverage

The initial native local-chain target is wasmd **v0.55.1**, commit
`fec61cbfb2e1dcfd2b1e86c9fb0a2bc7a9b3a223`, with **libwasmvm 2.2.4**. It builds
with Go **1.23.8**; the host's Go 1.27 produced an upstream dependency linker
failure (`encoding/json.unquoteBytes`). Example build:

```sh
git clone --branch v0.55.1 --depth 1 https://github.com/CosmWasm/wasmd.git
cd wasmd
test "$(git rev-parse HEAD)" = fec61cbfb2e1dcfd2b1e86c9fb0a2bc7a9b3a223
GOTOOLCHAIN=go1.23.8 make build
build/wasmd version --long --home /tmp/durable-version
build/wasmd query wasm libwasmvm-version --home /tmp/durable-version
```

Keep this development node isolated; these pins define a test target, not a
recommendation for public validator deployment. Native execution also requires
the wasmvm shared library installed by the Go module build.

The fulfillment contract currently has no `migrate` entry point. A suspended
workflow upgrade is therefore **not verified** by this suite. It needs an explicit
upgrade fixture retaining old handlers and a separate test of compatible versus
incompatible continuation migration. Other remaining measurements include a
variable-size checkpoint fixture and production-chain gas schedules.
