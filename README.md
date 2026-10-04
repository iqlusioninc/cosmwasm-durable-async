# Durable async workflows for CosmWasm

A prototype Rust runtime and procedural macro for workflows that suspend at explicit checkpoints and resume in later CosmWasm transactions. The macro compiles restricted `async` functions into serializable state machines. Execution uses ordinary CosmWasm entry points and contract storage.

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

## Why there is no Rust future executor

The `async fn` syntax looks like ordinary Rust async, but `#[durable_workflow]`
replaces the function before Rust lowers it into a native future. The macro
emits a serializable continuation enum and ordinary synchronous start/resume
handlers. There is no generated `Future` to poll.

Ordinary Rust async produces a `Future` whose `poll()` method advances execution.
An executor polls it and uses wake notifications to decide when to poll again.
Here, suspension instead saves the next step and the explicitly checkpointed
values in contract storage, then ends the invocation. A later contract invocation
loads that data and executes the next handler.

For example, the payment wait above becomes conceptually:

```text
start(order):
    save WaitingForPayment { order }
    dispatch payment request
    return

resume(payment_result):
    restore order from WaitingForPayment
    handle payment_result
    run the next segment until another wait or completion
    save the next continuation or terminal outcome
    return
```

This is pseudocode for the transformation, not a second public API. No Wasm stack,
waker, or native future survives between invocations. Only serialized contract
state survives; each invocation executes ordinary synchronous code in the chain's
Wasm runtime.

Scheduling is still necessary. In the execute-based example, a service or relayer
submits a callback transaction; the durable runtime authenticates it and dispatches
the saved continuation. An IBC adapter can instead resume from a chain-delivered
acknowledgement or timeout. The library does not run a background scheduler: if no
callback or expiration transaction arrives, the workflow remains suspended.

| Responsibility | Component |
| --- | --- |
| Remember where execution stopped | Checkpoint and continuation tag in contract storage |
| Trigger another invocation | Service callback transaction, adapter callback, or explicit expiration |
| Validate the event and select the next segment | `durable-runtime` |
| Execute the synchronous segment | CosmWasm VM, inside the transaction |

A segment's writes and outbound messages share its transaction's commit/rollback
boundary. The full workflow spans transactions: a later failure does not undo
earlier committed effects.

This is why the supported awaits are restricted to our typed durable operations.
An arbitrary Rust future, such as an async network client or Tokio timer, has no
adapter-defined callback and serializable continuation in this system.

## Errors, retries, and timeouts

An operation failure and an invocation failure have different consequences.
A valid remote error becomes `Err(WaitError::Remote { code, message })` at the
await. A deadline expiration becomes `Err(WaitError::Timeout)`.

With `.await?`, the error is converted into the workflow's application error and
stored as terminal `Failed`. Without `?`, application code receives the result
and can handle it explicitly. For example, using application-defined operation,
request and helper types:

```rust
let result: Result<Payment, WaitError> = ctx
    .wait::<PaymentReceived>(request)
    .checkpoint(order)
    .await;

handle_payment_result(ctx, order, result)
```

The synchronous helper can distinguish payment failure from timeout. Recovery
requiring another durable wait must follow the supported straight-line workflow
syntax; arbitrary branching around awaits is not supported.

A terminal application failure is persisted through a successful invocation.
Storage writes made earlier in that segment commit alongside `Failed`, provided
the enclosing transaction or receipt succeeds. An application error is therefore
not a request to roll back the segment. Earlier committed remote effects are not
undone either.

### Invocation failure and retry

An unauthorized, stale, malformed or oversized callback is rejected without
advancing the wait. A panic, gas exhaustion, runtime error or failing ordinary
outbound message aborts the transaction, restoring the previous continuation and
rolling back that transaction's application writes. Contract entry points must propagate
runtime errors for this rollback guarantee to hold.

After a failed resume transaction, the service can resubmit a valid authenticated
callback for the same wait before its deadline. This retries local continuation
execution; it does not require repeating the remote operation. There is no
automatic retry scheduler, and a committed terminal `Failed` is not a retryable
waiting state.

### Deadlines and late results

The operation adapter sets a finite block-height or timestamp deadline. Callback
acceptance is strictly before that deadline; at or after it, anyone can submit
`Expire { workflow_id, wait_sequence }`. Expiration executes the continuation with
`WaitError::Timeout` and can itself fail and roll back like any other resume.
Someone must submit the transaction: the runtime has no background timer.

The committed wait sequence prevents duplicate or stale callbacks and expiration
calls from advancing a consumed wait. A late result cannot revive a terminal
workflow. IBC adapters must define how packet acknowledgements and protocol
timeouts map onto this local deadline; a packet timeout must not bypass the
runtime's deadline checks.

Expiration does not cancel a remote action, prove that it never happened, or
refund funds automatically. An action may already have completed when its result
arrives too late. Applications needing reconciliation or remote-operation retries
must define operation IDs, idempotency, status queries and compensation explicitly.

## Run the prototype

Install rustup; `rust-toolchain.toml` selects Rust 1.99.0 and installs the Wasm target, rustfmt, and clippy. The pinned compiler also makes compile-failure snapshots reproducible. Then run:

```sh
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
rustup toolchain install 1.85.1 --profile minimal --component clippy
scripts/install-binaryen.sh
scripts/check-wasm.sh
```

The checked Wasm artifact is `target/artifacts/fulfill_example.wasm`, with a SHA-256 sidecar. The build uses pinned Binaryen 123 to normalize modern Rust output for the CosmWasm 2.2 VM. The [compiled-Wasm validation guide](docs/wasm-validation.md) describes the five gas-metered VM tests, tooling, and evidence limits. Validate against the intended chain before deployment. Dependencies are locked to CosmWasm 2.x and cw-multi-test 2.x in `Cargo.lock`.

The workspace contains:

| Package | Purpose |
| --- | --- |
| `durable-runtime` | Persistent lifecycle, authenticated callbacks, deadlines, limits, version registry, and explicit migration |
| `durable-macro` | Checkpoint compiler and compile-time restrictions |
| `fulfill-example` | Two-wait contract and transactional integration tests |

## Checkpoints and restrictions

Every value needed after an await must appear in `.checkpoint(...)`. Checkpointed arguments and locals need explicit owned types implementing `Serialize + DeserializeOwned`; a checkpointed local must have a type annotation on its declaration. Values moved into an operation request cannot also be checkpointed. `.checkpoint()` is valid when no local values need to survive.

V1 accepts straight-line functions returning `Result<Output, Error>` and waits written as simple local initializers. Only `.wait::<Operation>(request).checkpoint(...).await` is durable; arbitrary futures, loops, nested awaits, concurrent waits, and generic workflow definitions are excluded. Synchronous helpers may perform application logic and storage access. Each segment receives a fresh `WorkflowCtx`; references and invocation capabilities cannot cross an await.

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

There is no background executor, exactly-once off-chain delivery, automatic compensation, or cancellation. Runtime limits bound active instance counts and serialized payload sizes, but synchronous segment execution remains subject to chain gas limits. Finite deadlines and explicit expiration provide a resolution path; they do not guarantee someone submits a successful transaction. Terminal records are retained and consume storage.

## Design and verification scope

The [design specification](docs/superpowers/specs/2026-10-03-cosmwasm-durable-async-design.md) explains the continuation and transaction model. The [implementation plan](docs/superpowers/plans/2026-10-03-cosmwasm-durable-async.md) records the package interfaces and verification tasks.

Tests cover generated workflows, compiler restrictions, lifecycle validation, callback authorization, deadlines, failure records, and retained versions. Transactional integration uses cw-multi-test to verify rollback after outbound message failures. The compiled-Wasm suite additionally verifies lifecycle execution and gas exhaustion in CosmWasm VM 2.2.2 with mock host dependencies. It does not supply chain transaction rollback or dispatch outbound messages. The prototype has no production IBC adapter or external scheduler. Same-transaction callbacks may occur if a service resolves immediately; adapters requiring a later transaction must enforce that additional policy.
