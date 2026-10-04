# Durable async workflows for CosmWasm

This specification proposes a Rust library and procedural macro for CosmWasm contracts that suspend workflows and resume them in later transactions. Developers write restricted `async fn` functions with explicit checkpoints. The macro generates serializable continuations; the runtime authenticates events and executes synchronous segments. No chain modification or persistent Wasm memory is required.

The intended reader is a contract or library developer evaluating the architecture before implementation. The design follows the agreed requirements: ordinary-looking Rust async syntax, documented restrictions, explicit checkpoints, one outstanding wait per workflow, authenticated callbacks, retry after failed transactions, and expiration triggered by a transaction. The APIs below are proposed interfaces, not existing CosmWasm features or implemented code.

## Scope and success criteria

The first version must allow a contract to start a workflow, save its state at an external wait, and reconstruct its next segment from contract storage in a later invocation. Multiple independent instances may run concurrently. Each instance has at most one outstanding wait.

Success means a sequential two-wait workflow behaves identically after Wasm memory is discarded between every invocation. Unauthorized, malformed, stale, and duplicate callbacks must never advance it. A failed resume transaction must leave the original wait and continuation intact. A committed completion or workflow failure must remain queryable.

The first version excludes arbitrary Rust futures, Tokio integration, task spawning, concurrent waits, nested durable workflows, loops, and control flow containing awaits. It also excludes automatic background execution, a generic external scheduler, cancellation, automatic compensation, and automatic migration of suspended code. Contract developers may implement application-specific compensation as subsequent workflow operations.

## Execution model

CosmWasm invokes entry points synchronously. Contract storage persists across transactions; local variables and Wasm memory do not. A normal Rust executor cannot make compiler-generated futures durable because their hidden layouts are not a stable serialization format.

The macro consumes the annotated async function and replaces it with generated start and resume functions. It does not retain that function as a native Rust future. An await marks a segment boundary. The runtime executes one segment until it suspends, completes, or returns a workflow error. It never polls a saved future and has no background event loop.

An external service, another contract, an IBC callback, or an explicit execute caller causes a new invocation. That invocation supplies the result and pays transaction costs according to the chain's ordinary rules. The runtime cannot guarantee eventual delivery or progress without an external actor.

Each segment is part of one transaction. The full workflow spans multiple transactions and is not atomic. Earlier committed transfers and storage writes survive a later workflow failure.

## Authoring syntax and checkpoint semantics

```rust
#[durable_workflow(kind = "fulfill", version = 1)]
async fn fulfill(ctx: WorkflowCtx, order: Order) -> Result<Receipt, FulfillError> {
    let payment: Payment = ctx
        .wait::<PaymentReceived>(PaymentRequest { order_id: order.id })
        .checkpoint(order)
        .await?;

    let shipment: Shipment = ctx
        .wait::<ShipmentReceived>(ShipmentRequest {
            order_id: order.id,
            payment,
        })
        .checkpoint(order)
        .await?;

    Ok(Receipt { order_id: order.id, shipment })
}
```

`PaymentReceived` and `ShipmentReceived` are application-defined operation adapters. They define their request and successful result types, resolution policy, deadline, and any messages used to initiate the operation. This example assumes `order.id` is copyable, so constructing a request does not move `order` before the checkpoint.

The wait request is evaluated once in the segment that suspends. Values moved into the request are no longer available to checkpoint. The checkpoint captures the owned values named in its argument list after request construction. Resumption restores those names and binds the awaited result before executing the remaining source statements. A variable omitted from the checkpoint is out of scope after that await unless it is freshly declared there. Request data and continuation data are separate records; the application must explicitly checkpoint anything it needs later.

For the first version, each checkpoint accepts a list of unique local identifiers, including an empty list. Checkpointed function arguments must have explicit types in the signature. Checkpointed locals must have explicit type annotations on their declarations. The macro uses these written types to generate state fields and requires them to implement `Serialize + DeserializeOwned`. This avoids relying on type inference unavailable to a procedural macro. Successful outputs and durable failure records have the same serialization requirements.

Only top-level statements are accepted. A wait must appear as the entire right-hand side of a simple local binding, optionally followed by `?`. Without `?`, the binding receives `Result<Operation::Output, WaitError>` and synchronous helper code may inspect it. With `?`, `WaitError` must convert into the workflow error type. Ordinary fallible synchronous statements may also use `?`.

V1 rejects awaits in branches, loops, closures, macros, helper functions, and nested expressions. It rejects destructured checkpoint declarations, implicit checkpoint types, generics on workflow functions, async helper calls, and borrow-based checkpoint types. Synchronous helper calls are allowed, but they cannot suspend. Helpers can encapsulate branching and application logic within one segment.

The prototype additionally rejects all workflow-body macro invocations, conditional `cfg`/`cfg_attr` attributes on arguments and statements, and original `self::` or `super::` paths. Synchronous helpers can use these features; workflow code can use `crate::` paths. These explicit restrictions prevent opaque macro expansion and relocation into a generated module from silently changing source behavior. The context parameter cannot be shadowed. `Input` and `Workflow` are reserved local binding names because the generated module exports those names; parent types with those names must retain their original meaning in generated code.

`WorkflowCtx` is a reserved capability supplied anew for each segment. It offers access to that invocation's environment, API, querier, and storage through scoped methods. It is never checkpointed. `Deps`, `DepsMut`, references, storage handles, and capabilities carrying invocation lifetimes cannot survive a suspension. Generated separate function scopes and Rust's ownership checks enforce these restrictions; serialization bounds are an additional check, not a complete substitute for them.

## Generated continuation and components

The conceptual continuation for the example is:

```rust
enum FulfillV1State {
    WaitingForPayment { order: Order },
    WaitingForShipment { order: Order },
}
```

`payment` is consumed while preparing the second operation and is not checkpointed, so it does not appear in the second state. Terminal results belong to the instance record rather than this continuation enum.

The implementation has four components:

| Component | Responsibility | Dependencies |
| --- | --- | --- |
| Procedural macro | Validate syntax and generate state types and segment handlers | Rust syntax parsing and runtime interfaces |
| Runtime library | Persist instances, correlate waits, validate lifecycle, and dispatch segments | CosmWasm storage and generated workflow registry |
| Operation adapters | Define typed requests, result decoding, authorization, deadlines, and outbound messages | Application logic and runtime interfaces |
| Contract integration | Route execute, IBC, timeout, and query entry points | Runtime library and application authorization |

The macro generates code compatible with the runtime version it targets. A generated registry maps `(workflow kind, definition version)` to start and resume handlers. The explicit kind is independent of the Rust function name, so a contract can retain old and new versions as differently named functions. Unknown kinds, unsupported versions, and unknown continuation tags are runtime errors. Definition versions cover continuation encoding and operation result schemas, not just public function names.

## Persistent records and identifiers

An instance record contains:

- A contract-local monotonic workflow ID.
- A workflow kind and definition version.
- The creator and start height for observability; creator identity is not callback authorization.
- A status: `Waiting`, `Completed`, or `Failed`.
- For `Waiting`, the encoded continuation, expected continuation tag, and active wait record.
- For `Completed`, the encoded successful output.
- For `Failed`, the encoded application error.

A wait record contains its sequence number, operation kind, encoded request, callback policy, deadline, and expected result schema selected by its generated handler. Its externally visible correlation key is `(workflow_id, wait_sequence)`; routing also includes the contract address. Sequences increase monotonically within the instance and never wrap or repeat. Counter overflow rejects the operation.

Storage keys use a runtime-specific namespace distinct from application storage. Each `(workflow kind, definition version)` pair must be unique within the registry; different versions of the same kind coexist. V1 uses the CosmWasm JSON serialization helpers for records, continuation payloads, requests, outputs, and application errors. Schema compatibility belongs to the immutable definition version. For v1, terminal records remain stored and IDs are never reused. State and result retention therefore consume contract storage; contract administrators must account for this when authorizing starts. Pruning is a later feature requiring permanent ID non-reuse and clear query behavior.

## Operation adapters and callback protocol

A durable operation adapter defines serializable request and output types and two synchronous functions: prepare the request, and validate/decode its resolution. Preparation receives the allocated correlation key and fresh context and returns the persisted request metadata, resolver policy, required deadline, and outbound messages. It must not start an off-chain request directly; any off-chain action is triggered by committed chain messages or observed state/events.

The ordinary execute callback envelope is conceptually:

```rust
Resume {
    workflow_id: WorkflowId,
    wait_sequence: u64,
    outcome: OperationOutcome,
}
```

`OperationOutcome` is either encoded success data or a structured remote error. Size limits are enforced before decoding. The operation's expected handler selects the concrete output type; callback data cannot choose a decoder, workflow version, or continuation target.

For execute callbacks, the saved resolver policy contains a specific validated sender address. The runtime compares it with `MessageInfo.sender`. For IBC, a separate contract adapter verifies the appropriate channel and packet correlation from the chain-provided callback before producing a trusted resolution event. Merely being called through an IBC entry point does not authenticate a particular workflow result. V1 does not provide a public execute field that can claim to be trusted IBC input.

On resolution, the runtime:

1. Loads the instance and verifies that it is waiting on the supplied sequence.
2. Validates the actual caller or trusted callback context against the saved policy.
3. Checks that the deadline has not been reached, then validates and decodes the result.
4. Marks the old wait consumed in storage before running application continuation code.
5. Restores checkpoint values and executes the next segment with fresh context.
6. Persists its next wait or terminal result and returns its response.

While a segment runs, its consumed wait has an internal transition marker and is unavailable for resolution. This marker is never a committed instance status: before a successful handler return, it must be replaced by a new wait or terminal record. The storage update in step 4 and all subsequent updates share the transaction. If the transaction aborts, the old wait is restored. Consuming it before returning messages also prevents callbacks within the same transaction from reusing it. Wrong sequences, callbacks after termination, and duplicate resolutions return runtime errors without progressing the workflow. Thus progression happens at most once per committed wait; this does not promise exactly-once delivery or off-chain effects.

The runtime supports later-transaction callbacks, but cannot force a target contract to defer a callback. If a target synchronously calls back after messages are dispatched, the same authentication and correlation rules apply. Adapters requiring a later transaction must enforce that policy themselves. Native submessage `reply` handling is not a durable result source in v1.

## Transaction boundaries and failures

The runtime saves a continuation and active wait before returning initiation messages. Both commit together with the rest of the transaction. A failure elsewhere in the transaction, including failure of an ordinary outbound message, rolls back that save and all local changes. This means no durable instance is waiting for an operation that this transaction failed to initiate on-chain.

There are two failure domains:

| Failure | Handler behavior | Committed workflow state |
| --- | --- | --- |
| Authorized remote operation reports an error | Resume with `Err(WaitError)` | Whatever the continuation does next |
| Workflow propagates an application error | Record `Failed` and return a successful contract response | Terminal failure and this segment's storage writes |
| Invalid callback, failed decoding, or missing handler | Return contract error | Original wait remains |
| Panic, gas exhaustion, or failing response message | Transaction aborts | Original wait remains |

Application errors are deliberately recorded as outcomes, not propagated as transaction errors. Consequently, storage writes already made in that segment commit if the whole transaction succeeds. Contract integration must preserve this distinction: it cannot return an error after recording a terminal failure if that record is meant to persist. Durable failure is not a segment rollback mechanism.

V1 initiation messages use ordinary transaction failure semantics. Reply-based recovery of failed initiation messages is excluded. Emitted events summarize workflow ID, wait sequence, and transitions; queries and persisted records are authoritative. Event payloads should avoid unnecessary checkpoint contents.

## Deadlines and expiration

Every operation has a finite deadline, supplied explicitly or resolved from contract configuration during preparation. An adapter uses either block height or block time for that wait. Preparation rejects deadlines already reached. Callbacks are accepted only before the deadline; expiration is allowed at or after it. The same boundary rule applies in both paths.

Anyone may invoke `Expire { workflow_id, wait_sequence }`. The runtime verifies the stored deadline and correlation key, consumes the wait, and resumes the continuation with `WaitError::Timeout`. This is a transaction and can fail or exhaust gas like any other resume. No scheduler is built into the runtime.

A callback and expiration race is resolved by transaction ordering. A stale expiration never expires a newer wait. Expiration does not cancel a remote action or refund transfers automatically; the remote action can still finish after the local wait expires. Application protocols need their own reconciliation where that matters.

## Limits and contract integration

Contract configuration establishes finite limits for active workflows, checkpoint bytes, request bytes, callback bytes, and terminal result bytes. The runtime measures serialized data and rejects oversized values before storing them. Specific values are contract deployment choices; library tests must exercise configured boundaries. A contract also controls who may start workflows and what funds or fees a start requires.

Single-segment execution remains subject to chain gas limits. Without loops or asynchronous recursion in the durable dialect, the macro does not introduce an unbounded executor loop, but synchronous helpers can still exhaust gas. Resumption attempts must supply sufficient gas through normal chain mechanisms.

The contract exposes start handlers, authenticated execute callbacks, permissionless expiration, and queries for instance status and results. It optionally routes chain-supported IBC callbacks through operation-specific adapters. The runtime cannot invent an entry point unavailable on the selected chain. Production integration must select and test a concrete CosmWasm SDK and chain feature set.

## Definition versions and upgrades

For v1, workflow versions are explicit positive integers. Changing statement order, checkpoint fields, operation types, continuation tags, or serialized schemas requires a new version. Old versions remain registered while instances using them are active. Published versions are immutable even when a change appears source-compatible.

Generated continuation tags are deterministic within an immutable definition. They are not source line numbers. Existing instances keep their saved definition version on every resume. The callback cannot override it.

A contract migration must retain compatible old handlers or explicitly transform active records into new schemas. The runtime does not automatically migrate Rust execution state. Dropping a handler for an active version prevents those instances from resuming, so contract migration validation must check active version counts. V1 maintains those counts on start and terminal transitions. Migration authorization follows the contract's ordinary administrative rules.

## Verification requirements

Macro compile fixtures must demonstrate valid multiple checkpoints and enforce explicit types, ownership, supported await positions, and serialization bounds. Rejected fixtures must cover non-checkpointed variables used after an await, moved values, references, unknown operations, branches containing awaits, loops, and ordinary future awaits. Assertions should use stable diagnostic content where possible.

Runtime integration tests must start a workflow, reconstruct the contract with fresh memory, resume through two separate calls, and query its terminal result. Each suspension must round-trip its encoded continuation. Tests must include an awaited result handled without `?` and an application error propagated with `?`.

Authorization and lifecycle tests must cover wrong senders, wrong wait sequences, malformed and oversized payloads, duplicate callbacks, callbacks after termination, old timeout messages, and unsupported definition versions. Test expiration immediately before and exactly at both height and time deadlines. Simulate reordered callback and expiration transactions.

Transactional tests must verify that a failed initiation message leaves no newly committed wait, and that a resume which performs storage writes before a failing response message leaves the original wait and prior storage intact. An application error recorded with a successful response must instead commit its failure record and segment writes. Direct in-memory handler calls do not establish transaction rollback; use a CosmWasm transactional integration harness and a Wasm target test environment for host-specific behavior such as gas exhaustion.

Upgrade tests must retain a v1 handler while new v2 instances run, reject a migration that drops an active handler, and exercise an explicit state migration. IBC adapter tests belong to adapters and require the chain callback semantics they authenticate; the generic execute path is sufficient for the initial runtime prototype.

## Delivery boundary

The first implementation should include the runtime, procedural macro, a sample contract with two authenticated execute-based waits, and the verification suite above. The core runtime supports trusted resolution contexts so IBC adapters can be added independently, but a production IBC adapter is outside that first delivery.

The initial prototype establishes that explicit checkpoint compilation and durable resumption are practical. It makes no claim to support arbitrary Rust async code. Branches, loops, nested workflows, concurrent waits, cancellation, pruning, and automatic compensation require later designs without weakening existing version or transaction semantics.
