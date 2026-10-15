# Durable async workflows for CosmWasm

A prototype Rust compiler and runtime for workflows that suspend at explicit checkpoints. The macro compiles restricted `async` functions into serializable state machines. Two execution paths are available: transaction-driven contract storage, and an experimental native scheduler with task escrow and automatic block execution.

```rust
#[durable_workflow(kind = "fulfill", version = 1)]
async fn fulfill(ctx: WorkflowCtx, order: Order) -> Result<Receipt, WorkflowError> {
    let payment: Payment = ctx
        .wait::<PaymentReceived>(PaymentRequest { order_id: order.id })
        .checkpoint(order)
        .await?;

    record_payment(ctx, order.id, &payment)?;

    let shipment: Shipment = ctx
        .wait::<ShipmentReceived>(ShipmentRequest { order_id: order.id, payment })
        .checkpoint(order)
        .await?;

    Ok(Receipt { order_id: order.id, shipment })
}
```

The complete function, operation adapters, and contract entry points are in [the example contract](contracts/fulfill/src/lib.rs). The attribute replaces the function with a `fulfill` module exporting `Input` and `Workflow`; a call to `durable_runtime::start::<fulfill::Workflow>` starts an instance. There is no native future to call or poll.

## Run the prototype

Install rustup; `rust-toolchain.toml` selects Rust 1.99.0 and installs the Wasm target, rustfmt, and clippy. The pinned compiler also makes compile-failure snapshots reproducible. Then run:

```sh
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build -p fulfill-example --release --target wasm32-unknown-unknown --locked
```

The Wasm artifact is `target/wasm32-unknown-unknown/release/fulfill_example.wasm`. A successful build alone does not establish compatibility with a particular chain's VM; validate the artifact and gas behavior against that chain before deployment. Dependencies are locked to CosmWasm 2.x and cw-multi-test 2.x in `Cargo.lock`.

The workspace contains:

| Package | Purpose |
| --- | --- |
| `durable-runtime` | Persistent lifecycle, authenticated callbacks, deadlines, limits, version registry, and explicit migration |
| `durable-macro` | Checkpoint compiler and compile-time restrictions |
| `fulfill-example` | Two-wait contract and transactional integration tests |
| `native-async` | Native poll ABI, funding SDK, and persisted reference scheduler |
| `native-fulfill-example` | Generated workflows exposed through the native poll and funding interfaces |
| `native-dca` / `mock-exchange` | Finite scheduled trades and an immutable fixed-price local test fixture |

## Checkpoints and restrictions

Every value needed after an await must appear in `.checkpoint(...)`. Checkpointed arguments and locals need explicit owned types implementing `Serialize + DeserializeOwned`; a checkpointed local must have a type annotation on its declaration. Values moved into an operation request cannot also be checkpointed. `.checkpoint()` is valid when no local values need to survive.

Workflows return `Result<Output, Error>` and durable waits are simple local initializers. Statement-level branches, `while`, `loop`, and unlabeled `break`/`continue` are supported. Control-flow mode requires explicitly typed owned locals, no shadowing, and captures of outer locals on every join/backedge path; it does not infer liveness. `for`, expression-valued durable control flow, arbitrary futures, concurrent waits, and generic workflow definitions remain excluded. Synchronous helpers may perform application logic and storage access. Each segment receives a fresh `WorkflowCtx`; references and invocation capabilities cannot cross an await.

Durable helpers use `ctx.call::<child::Workflow>(child::Input { ... }).checkpoint(parent_fields...).await?`. They share the parent's task and escrow; explicit parent captures and child state survive each wait. Helper callers use the typed control-flow dialect. Immediate child completion continues in the same poll, and sequential helper loops dispatch iteratively. Composition is acyclic and limited to 16 nested calls. Changing a child's definition requires a new containing parent version. Publicly exported workflows need public output/error types; generated workflow visibility follows the original function.

Workflow bodies also exclude macro invocations, conditional `cfg`/`cfg_attr` attributes, and source paths beginning with `self::` or `super::`. Put such logic in synchronous helpers or use `crate::` paths. These restrictions prevent module relocation or opaque macro expansion from changing name resolution. The context parameter cannot be shadowed, and `Input` and `Workflow` are reserved local binding names; same-named parent types remain usable.

With `.await?`, a remote operation error or timeout becomes an application error through `From<WaitError>`. Without `?`, the binding receives `Result<Operation::Output, WaitError>` so synchronous logic can handle it. Outputs and application errors must also be serializable. Operation decoders must be pure: the runtime validates a result before execution, then the generated handler decodes it again.

The function's kind and version identify an immutable definition. Changing checkpoint layout, statement order, or operation schema requires a new version. Keep old generated handlers registered while old instances remain active. The runtime can check active-version coverage and explicitly migrate a waiting record; contract administrators must authorize such migrations and provide a compatible transformed continuation.

## Starting and resuming the example

Instantiate the example with payment and shipment service addresses and a positive block deadline. Its instantiator becomes the owner; only that owner may start workflows. This example does not accept funds.

A start execute message has this shape:

```json
{"Start":{"order":{"id":42}}}
```

The response data contains the allocated workflow ID. The contract emits a `ServiceMsg::Begin` to the payment service containing the correlation key and a JSON request encoded as CosmWasm `Binary`. The service saves that key and later submits a callback:

```json
{"Resume":{"workflow_id":1,"wait_sequence":1,"outcome":{"Success":"eyJyZWZlcmVuY2UiOiJwYWlkIn0="}}}
```

Here `Success` contains base64 JSON for `Payment { reference: "paid" }`. It is accepted only from the configured payment service. The workflow restores its `order`, records the payment, and initiates the shipment operation with sequence 2. A callback from the shipment service containing a `Shipment` completes the instance and stores a `Receipt`.

Services must implement the example's `ServiceMsg` protocol. The example does not include a production service or relayer. Its test services accept requests without resolving them; separate test transactions then supply the callbacks. Query `{"Instance":{"workflow_id":1}}` for status and result. The resolver address and current sequence come from the saved wait, never from an unauthenticated claim in a callback.

A service can report a failure with `{"Error":{"code":"declined","message":"payment refused"}}` as the outcome. Anyone can call `{"Expire":{"workflow_id":1,"wait_sequence":1}}` once the saved deadline has been reached. Timeouts require a transaction; the chain does not wake the workflow on its own. Callback acceptance is strictly before the deadline, and expiration is at or after it.

## Transaction and delivery guarantees

Each segment is atomic within its transaction. A saved wait and its outbound messages commit together. A failing transaction restores the old continuation and application storage, allowing a retry. Authenticated duplicate or stale callbacks cannot advance a consumed wait again.

A propagated application error is recorded as terminal `Failed` while the entry point returns successfully. Storage writes made earlier in that segment commit with that failure record. Application failure does not roll back earlier transactions or reverse remote actions. If the entire transaction subsequently fails, the failure record rolls back too.

The transaction-driven example has no background executor or cancellation. Neither execution path promises exactly-once off-chain delivery or automatic compensation. Runtime limits bound active instance counts and serialized payload sizes, but synchronous segment execution remains subject to chain gas limits. The transaction-driven path requires explicit expiration transactions and retains terminal records; the native path has bounded expiry/pruning and prepaid retention.

## Design and verification scope

For a runnable native example, see the [local DCA tutorial](contracts/native-dca/README.md).
From the repository root, `bash scripts/native-dca-demo.sh all` builds the test
artifacts and runs happy-path, gas funding, liquidity recovery, cancellation and
process-restart scenarios against a patched local validator. First-run downloads,
prerequisites and reuse instructions are in the tutorial and the
[adapter guide](integration/wasmd/README.md#local-dca-scenarios). This uses only
local test coins and deterministic test-only block permits; it is not a market,
free-running cadence benchmark or production deployment.

The [native CosmWasm async design](docs/superpowers/specs/2026-10-11-native-cosmwasm-async-design.md) describes chain-managed tasks, block scheduling, prepaid gas, and durable wakeups. The [dynamic implementation plan](docs/superpowers/plans/2026-10-11-native-async-runtime.md) tracks implementation and acceptance gates; the design's full production validation criteria are not implied by unit tests.

The [native runtime](crates/native-async/README.md) implements task-specific escrow, bounded deterministic scheduling, durable inboxes/timers, atomic poll rollback with fee settlement, storage leases, refunds, and migration gating. The [native example](contracts/native-fulfill/README.md) exposes registration, authenticated callbacks, and the mandatory funding/query interface. The pinned [wasmd adapter](integration/wasmd) executes this ABI through privileged Sudo with SDK gas meters. Run Rust integration tests with `cargo test --workspace --all-features --locked`; real Wasm/chain checks are a separate suite.

Native `gas_per_poll` is the total attempt cap, including bounded host overhead. Funding never grants execution authority or automatically retries a paused task. Controllers can prepay lease/quota growth and update the poll cap, price ceiling, or refund recipient; resumption remains explicit. Lease extension never extends an existing operation deadline.

The [design specification](docs/superpowers/specs/2026-10-03-cosmwasm-durable-async-design.md) explains the continuation and transaction model. The [implementation plan](docs/superpowers/plans/2026-10-03-cosmwasm-durable-async.md) records the package interfaces and verification tasks.

Tests cover generated workflows, compiler restrictions, lifecycle validation, callback authorization, deadlines, failure records, retained versions, native scheduling, and escrow accounting. Transactional integration also uses cw-multi-test. There is no production IBC adapter, unrestricted Rust Future persistence, guaranteed per-block service class, or production gas calibration. Same-transaction callbacks may store an immediate result; native scheduler-generated wakeups cannot recursively poll and are eligible no earlier than the next block.
