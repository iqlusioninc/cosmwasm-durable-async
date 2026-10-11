# Native Async Runtime Dynamic Implementation Plan

> **For agentic workers:** Use parallel-agent dispatch for independent ownership domains and subagent-driven review for completed milestones. Maintain the execution ledger as dependencies change.

**Goal:** Build the recommended native-continuation runtime through executable registration, deterministic block scheduling, durable wakeups, paid execution/storage, and a pinned wasmd integration with real Wasm tests.

**Spec:** ../specs/2026-10-11-native-cosmwasm-async-design.md

**Architecture:** Native host state owns continuations and escrow. Wasm receives a private poll request and returns an encoded outcome plus outbound messages; the host stages the wait before dispatching those messages. A Rust reference implementation supplies deterministic state-machine tests; a Cosmos SDK/wasmd adapter supplies actual gas meters and cached multistore behavior.

**Tech stack:** Current locked Rust workspace; pinned wasmd v0.55.1 / Cosmos SDK v0.50.12 / wasmvm v2.2.4, subject to verified upstream APIs.

## Global constraints

- Preserve the original checkout and continue in `/private/tmp/cosmwasm-native-async.i7sHwm` from b1329ac.
- Host task ownership comes from the executing contract; sponsor funding grants no control.
- Register at H, first poll no earlier than H+1; at most one poll per task per block; scheduler-generated wakeups are H+1.
- All selection, deadlines, gas prices, and counters are consensus-deterministic.
- No scan of every sleeping task. Queue visits, due timers, expiration, pruning, execution gas and state bytes are bounded.
- Poll effects and continuation/messages commit atomically; consumed gas is charged even when poll effects roll back.
- Accepted operation results persist across failed polls and remain valid after their operation deadline. Task expiry is an independent hard limit.
- Application failure commits as a terminal outcome. Out-of-gas/trap/message failure pauses without automatically retrying.
- Await crossing retains owned serialized state, never invocation-scoped references or capabilities.
- Active tasks block contract migration. Task IDs never repeat after pruning.
- Persistent Wasm snapshots and guaranteed reserved per-block capacity are alternative/later tracks explicitly excluded from the recommended first runtime in the spec. Do not misrepresent them as implemented.

## Dynamic queue

| Milestone | Ownership | Depends on | Evidence gate |
| --- | --- | --- | --- |
| A: Runtime ABI and reference executor | native-async crate | Existing funding | State machine + accounting + resource-bound tests |
| B: Existing compiler bridge and executable example | New contract/bridge crate, narrow WorkflowCtx constructor if needed | A frozen ABI | Generated two-wait workflow runs through native poll ABI |
| C: Real-chain integration | integration/wasmd, separate pinned source checkout | A frozen ABI and upstream discovery | Go keeper/adapter tests executing built Wasm |
| D: Fault/restart/parity validation | New integration tests/scripts | A/B/C | Failure fees, state persistence, deterministic scheduling, migration guard |
| E: Authoring expansion | durable-macro/runtime | A/B integration stable | Branch/loop/helper semantics with compiler tests where feasible |
| F: Whole-system review/documentation | Root coordination + independent reviewers | All prior | Explicit implemented/unverified boundaries and reproducible commands |

## Task A: Runtime ABI and reference executor

Own `crates/native-async` only. Publish a small `execution.rs` ABI first, then implement a persisted scheduler and fees/storage lifecycle using an explicit host adapter. Preserve existing funding tests or intentionally update their protocol assumptions with migration notes. Reference execution is not real VM metering: the adapter must report consumed gas and the state machine must bound/settle it correctly.

Required ABI concepts: executable definition kind/version/input; start/resume request including task ID and wait sequence; pending continuation with a height wait or authenticated operation wait; yield, complete, fail; bounded result delivery; separately encoded outbound Cosmos messages. Registration includes a code identity and owned-state limits. Preserve SDK helpers for funding/resume/cancel and add executable registration/query/delivery helpers.

Tests must demonstrate automatic block selection, one poll/block, H+1 registration/yield semantics, deterministic contract rotation, bounded timer/queue work, out-of-gas rollback with paid fee, authenticated inbox replay protection, timeout boundaries, expired task refunds/pruning, lease admission, no drain across tasks, and active-task migration gating. Commit code after focused/full tests, report exact tests and caveats.

## Task B: Compiler bridge and native example

Own a new bridge module/crate and `contracts/native-fulfill`; do not edit Task A files without coordination. Adapt the current generated `Workflow` trait into the frozen native poll ABI. Add a narrow safe WorkflowCtx constructor if required; it must take fresh invocation capabilities and cannot persist references. The contract's sudo poll handler returns the encoded native outcome/messages rather than dispatching outbound messages prematurely. Its public API registers a workflow and returns the host ID through reply handling, exposes mandatory funding/query management, and routes authenticated operation result delivery.

Provide a real Wasm artifact built with the pinned Rust toolchain and a reproducible normalization step if the selected VM requires it. Test successful two-wait fulfillment, malformed results, failure propagation, and code/definition mismatch through the bridge. Existing non-native prototype behavior must remain unchanged.

## Task C: Pinned Cosmos SDK/wasmd adapter

Own `integration/wasmd` and an isolated pinned wasmd source tree. Implement native custom message/query routing, host-owned task storage and module custody, post-transaction scheduler execution, cached poll rollback, actual SDK/VM gas accounting, fee settlement outside the discarded poll cache, bounded host maintenance, and a migration guard. Favor the existing privileged sudo VM invocation carrying the poll ABI if sufficient; a new VM export is not required merely for naming.

Execute outbound messages only after validating/staging the outcome/wait. Freeze block budget rules so async work is part of an explicitly partitioned application allocation. Tests must use actual wasmd/SDK keepers and real compiled Wasm, not a fake that reports self-chosen gas. Provide pinned source/dependency versions and a reproducible bootstrap/patch or standalone integration module; no external deployment, merge, or publication.

## Tasks D–F: Adapt after evidence

The root writes exact follow-up briefs after A/B/C expose their concrete interfaces. Every gap from the spec remains in the ledger until implemented, verified, or explicitly reported as blocked; test failures create focused repair tasks rather than silent scope reductions. Review completed independent domains before integrating dependent work. Final tests include the original workspace suite, Wasm build/VM validation, Go integration tests, and repeatability/restart tests feasible in the local environment.

## Review focus

Result accepted before deadline but consumed after it; malicious host-message reentrancy during a poll; failure accounting outside child caches; global storage/admission exhaustion; new work generated late in the scheduler phase; task-ID and definition replay after cancellation/pruning; migration against suspended shared storage; actual rather than declared gas consumption.
