# Native Async Funding Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans for this first increment. Steps use checkbox syntax for tracking.

**Goal:** Implement the required async contract funding interface and a reference host escrow keeper, verified through actual simulated bank transfers and transaction rollback.

**Architecture:** Add an independent `native-async` crate containing the wire types and SDK helpers. A feature-gated reference host keeper owns task funding records; a cw-multi-test adapter exercises its transaction and bank integration. This is the funding slice of the native design, not a running block scheduler or wasmd patch.

**Tech Stack:** Existing pinned Rust toolchain, CosmWasm 2.x, serde, schemars 0.8, cw-multi-test 2.5.

**Spec:** ../specs/2026-10-11-native-cosmwasm-async-design.md

## Global constraints

- Every async-capable contract exposes a standard funding and balance-query interface backed by scheduler escrow.
- Top-up does not resume tasks or change their control/refund authority.
- Only real transferred native coins create escrow credit.
- All mutations execute in a host transaction cache; a bank or downstream failure rolls back the entire operation.
- No claim of VM gas calibration, automatic polling, storage lease pricing, or production chain integration in this increment.

## Review focus

- Funding another contract's task: keys derive from the authenticated executing contract.
- Phantom funding from JSON or retained business balances: SDK uses attached funds; host debits actual balances.
- Unauthorized retries and refunds: host checks recorded authority, forwarded by the trusted owning contract.
- Arithmetic overflow and exact expiry: reject before transfer; do not mutate state on failure.
- Failed messages after credit: integration tests assert bank balances and ledger rollback together.

## Task 1: Types and contract funding helpers

Files: create `crates/native-async/{Cargo.toml,src/lib.rs,src/types.rs,src/sdk.rs,tests/sdk.rs}`; update workspace membership and lockfile.

Interfaces: `AsyncExecuteMsg`, `AsyncQueryMsg`, `SchedulerMsg`, `SchedulerQuery`, `FundingConfig`, `TaskFunding`, `fund_task(info, task_id, denom) -> StdResult<Response<SchedulerMsg>>`, `query_funding(deps, env, msg) -> StdResult<Binary>`.

- [x] Write tests rejecting zero, mixed/duplicate/wrong denominations and verifying the forwarded contributor and actual attached amount.
- [x] Run tests and observe missing functionality, then implement types and helpers.
- [x] Run SDK tests, compile derived wire schemas, and build the SDK for Wasm.

## Task 2: Reference host keeper and bank integration

Files: create `crates/native-async/src/host.rs`, `tests/funding.rs`, `tests/support/mod.rs`.

Interfaces: host `Keeper` registers/funds/resumes/cancels task escrows through an explicit bank-transfer adapter and provides authoritative queries. `SchedulerMsg` has no arbitrary owning-contract target. A trusted host-only pause method supplies a saved runnable/waiting disposition; no public caller can mark a task runnable.

- [x] Write integration tests for registration, sponsor top-up, plain bank transfers, fund-and-resume, refund ownership, wrong-contract routing, expiry, insufficient balance, overflow, and rollback after downstream failure.
- [x] Run initial integration tests against stubs and observe behavioral failures; add boundary cases.
- [x] Implement host validation and checked accounting; transfer through the simulated bank inside the transaction cache.
- [x] Run targeted tests and the full workspace suite; run fmt/clippy and Wasm build.

## Task 3: Review and handoff

- [x] Document the contract wiring and the trusted-host boundary in the crate README and root README.
- [x] Review the whole increment against the specification and record deferred runtime work.
- [x] Commit the isolated branch after checks pass; retain the primary checkout's uncommitted design changes.

## Execution notes

The user's request to start building authorizes implementation in this session. The first increment is bounded to funding because this checkout contains neither wasmd nor the SDK runtime. Existing design changes were copied into an isolated local clone, leaving the original checkout unchanged. Block scheduling, polling ABI execution, gas charging after failed polls, durable inboxes, storage lease charging, and compiler expansion remain subsequent increments.

Independent review found no Critical or Important defect in the documented funding-only boundary. Its refund rollback test gap was addressed with a regression covering both a failed bank refund and a subsequent transaction failure. The trusted-adapter, immutable denomination/custody, and contract-asserted actor boundaries are documented in the crate README. Returning the host-assigned ID through contract reply handling remains an explicit application integration responsibility.

Final validation: 48 workspace test functions passed (including 24 trybuild fixtures within the compile test); all-feature/all-target Clippy and formatting checks passed; the default-feature SDK built for wasm32-unknown-unknown. Twelve new tests cover SDK and host funding behavior. No upstream dependency versions changed.
