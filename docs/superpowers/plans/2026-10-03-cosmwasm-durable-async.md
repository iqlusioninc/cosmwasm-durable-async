# CosmWasm durable async implementation plan

> **For agentic workers:** Use test-driven development and independent runtime and macro implementers against the interface below. Integration and a fresh whole-project review follow.

**Goal:** Build a working two-wait CosmWasm workflow that persists explicit checkpoints and resumes through authenticated calls in separate transactions.

**Architecture:** A procedural macro generates typed continuation enums and synchronous workflow handlers. The runtime persists instances and delegates each validated resolution through a versioned registry. An example contract and transactional tests exercise both components together.

**Tech Stack:** Rust 2021, CosmWasm 2.x, serde JSON, syn 2, quote, trybuild, cw-multi-test 2.x.

**Spec:** ../specs/2026-10-03-cosmwasm-durable-async-design.md

## Global constraints

- One outstanding wait per workflow; owned explicitly typed checkpoints only.
- Runtime-provided durable waits only; no branches with awaits, loops, or arbitrary futures.
- Authenticated callbacks, monotonically increasing correlation keys, and finite deadlines.
- Application failures commit terminal records; runtime errors abort transactions.
- Old workflow versions remain registered while active instances exist.
- No production IBC adapter or gas-metered VM claim in this prototype.

## Review focus

- A local moved into an operation request cannot subsequently be checkpointed.
- Callback payloads and resolver addresses must be validated before application execution.
- Outbound message failure must restore both continuation and application storage.
- Timeout equality and stale timeout messages must not advance the wrong wait.
- Registry changes must not silently strand active versions.

## Shared interfaces

All names below are exported by `durable_runtime`; it also reexports `serde` and `cosmwasm_std` so macro-generated code can use qualified paths.

```rust
type RuntimeResult<T> = Result<T, RuntimeError>;
struct Correlation { workflow_id: u64, wait_sequence: u64 }
enum Deadline { Height(u64), Time(u64) } // Time is Unix seconds.
enum Outcome { Success(Binary), Error { code: String, message: String } }
enum Resolution { Outcome(Outcome), Timeout }
enum WaitError { Remote { code: String, message: String }, Timeout }
struct PreparedWait {
    operation: String, request: Binary, resolver: Addr,
    deadline: Deadline, messages: Vec<CosmosMsg>,
}
enum Transition {
    Wait { state: Binary, wait: PreparedWait },
    Completed(Binary), Failed(Binary),
}
```

`WorkflowCtx<'a>` offers `storage(&mut self) -> &mut dyn Storage`, `api(&self) -> &dyn Api`, `querier(&self) -> &QuerierWrapper<'_>`, `env(&self) -> &Env`, `info(&self) -> &MessageInfo`, `correlation(&self) -> Correlation`, and `prepare<O: Operation>(&mut self, request: O::Request) -> RuntimeResult<PreparedWait>`. Preparation sets `operation` and serialized `request` itself; adapters provide resolver, deadline, and messages.

```rust
trait Operation {
    const KIND: &'static str;
    type Request: Serialize + DeserializeOwned;
    type Output: Serialize + DeserializeOwned;
    fn prepare(ctx: &mut WorkflowCtx<'_>, key: Correlation,
               request: Self::Request) -> RuntimeResult<PreparedWait>;
    fn decode(outcome: Outcome) -> RuntimeResult<Result<Self::Output, WaitError>>;
    // Default decode: JSON success or typed remote error.
}
fn decode_resolution<O: Operation>(resolution: Resolution)
    -> RuntimeResult<Result<O::Output, WaitError>>;
trait Workflow {
    const KIND: &'static str;
    const VERSION: u32;
    type Input: Serialize + DeserializeOwned;
    fn start(ctx: &mut WorkflowCtx<'_>, input: Binary) -> RuntimeResult<Transition>;
    fn validate(state: &Binary, resolution: &Resolution) -> RuntimeResult<()>;
    fn operation(state: &Binary) -> RuntimeResult<&'static str>;
    fn resume(ctx: &mut WorkflowCtx<'_>, state: Binary, resolution: Resolution)
        -> RuntimeResult<Transition>;
}
```

The macro turns the function name into a same-name module exporting `Input` and `Workflow`. Every non-context argument becomes a public `Input` field. It generates a private serde continuation enum with one variant per wait, separate segment scopes, and the `Workflow` implementation. It uses `to_json_binary` and `from_json`. `validate` decodes state and resolution without executing application code; `operation` returns the expected operation kind. Decoder functions must be pure because validation and execution both decode.

Runtime entry points:

```rust
fn start<W: Workflow>(deps: DepsMut, env: Env, info: MessageInfo,
    input: W::Input, limits: &Limits) -> RuntimeResult<(u64, Response)>;
fn resume(deps: DepsMut, env: Env, info: MessageInfo, workflow_id: u64,
    wait_sequence: u64, outcome: Outcome, registry: &Registry,
    limits: &Limits) -> RuntimeResult<Response>;
fn expire(deps: DepsMut, env: Env, info: MessageInfo, workflow_id: u64,
    wait_sequence: u64, registry: &Registry, limits: &Limits)
    -> RuntimeResult<Response>;
fn load_instance(storage: &dyn Storage, workflow_id: u64)
    -> RuntimeResult<Instance>;
fn active_count(storage: &dyn Storage, kind: &str, version: u32)
    -> RuntimeResult<u64>;
fn assert_supported_versions(storage: &dyn Storage, registry: &Registry)
    -> RuntimeResult<()>;
fn registration<W: Workflow>() -> WorkflowRegistration;
impl Registry { fn new(entries: Vec<WorkflowRegistration>) -> RuntimeResult<Self>; }
```

`Limits` has public `max_active: u64`, and `max_checkpoint_bytes`, `max_request_bytes`, `max_callback_bytes`, `max_result_bytes` as `usize`, with finite defaults. `Instance` has public `workflow_id`, `kind`, `version`, `creator`, `start_height`, and `status`. `Status` is `Waiting { state: Binary, wait: WaitRecord }`, `Completed { output: Binary }`, `Failed { error: Binary }`, plus an internal transition marker if needed. `WaitRecord` has public `sequence`, `operation`, `request`, `resolver`, and `deadline`. All wire records derive serde and Debug/PartialEq where sensible. RuntimeError is a thiserror error with StdError conversion and a Validation(String) variant for adapter checks.

## Task 1 Runtime lifecycle

**Files:** `crates/durable-runtime/src/{lib,types,context,registry,runtime}.rs`, `crates/durable-runtime/tests/lifecycle.rs`.

- [x] Write lifecycle tests against a handwritten two-step workflow; run and observe missing API failures.
- [x] Implement the shared interfaces, namespaced JSON storage, monotonic IDs, limits, active counts, and terminal results.
- [x] Implement authorized callback validation and deadline expiration before execution; test wrong sender, stale/duplicate sequence, payload limits, and height/time boundary equality.
- [x] Implement version coverage validation and an explicit waiting-state migration helper that preserves correlation/resolver/deadline and validates the target handler and operation identity.
- [x] Run `cargo test -p durable-runtime` and record results.

## Task 2 Checkpoint compiler

**Files:** `crates/durable-macro/src/{lib,parse,expand}.rs`, `crates/durable-macro/tests/{compile.rs,ui/*,workflow.rs}`.

- [x] Write compile and generated-workflow tests first, demonstrating a two-wait function; observe failing macro/API tests.
- [x] Parse explicit kind/version and async function syntax; validate the restricted dialect.
- [x] Generate `Input`, continuation states, and start/validate/operation/resume handlers using the shared interfaces.
- [x] Keep ordinary `?` application errors in segment closures; serialize them as Failed. Serialization/preparation/decoding errors stay RuntimeError.
- [x] Exercise typed checkpoints, an empty checkpoint, `.await` without `?`, moved values, omitted variables, implicit types, unsupported futures, loops, nested awaits, and unknown operations.
- [x] Run `cargo test -p durable-macro` and record results.

## Task 3 Contract and transactional integration

**Files:** `contracts/fulfill/src/lib.rs`, `contracts/fulfill/tests/transactions.rs`.

- [x] Write transactional tests for two separate callbacks and queryable output; run to observe missing contract behavior.
- [x] Implement owner-authorized start, operation adapters, callback/expiration routing, queries, and a registry containing the generated workflow.
- [x] Verify duplicate/unauthorized/malformed callbacks and remote errors/timeouts.
- [x] Verify failure of initiation and resume messages rolls back runtime and application storage; verify application failure commits both.
- [x] Verify simultaneous retained workflow versions and explicit migration; run `cargo test -p fulfill-example`.

## Task 4 Delivery and review

**Files:** `README.md`, `Cargo.lock`, verification logs in `scratch/`.

- [x] Document runnable commands, example syntax, ownership restrictions, callback shape, and actual prototype limitations.
- [x] Run `cargo fmt --all -- --check`, `cargo test --workspace`, and `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] Build the example for `wasm32-unknown-unknown` in release mode; report any platform limitation precisely.
- [x] Obtain a fresh independent whole-project review; fix important findings with regression tests.
- [x] Deliver source paths, test evidence, and remaining VM/IBC limitations without publishing externally.

## Execution notes

The user explicitly requested implementation after reviewing the written spec. Work proceeds in the supplied empty workspace, which has no existing repository or branch to isolate. Runtime and macro tasks may proceed in parallel after this shared interface is established. Their implementations use failing tests first. Integration starts once both compile.
