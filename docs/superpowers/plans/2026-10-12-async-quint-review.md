# All-open-PR Async Quint Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Executable, reproducible formal models and an evidence-grounded adversarial review covering all five open PRs without changing runtime code.

**Architecture:** Separate exact pricing, native scheduler, delayed service and IBC models; compose small verification profiles rather than inventing a merged implementation. Model guest execution abstractly while retaining host accounting and callback interleavings.

**Tech Stack:** Quint 0.33.0, project-local npm tool lock, TypeScript evaluator for portability; TLC/Apalache when executable locally.

**Spec:** `docs/superpowers/specs/2026-10-12-async-quint-review-design.md`

**Execution status (2026-10-12 local):** Tasks 1–3 implemented and independently
reviewed. Task 4, whole-branch review and scoped F1/F2 re-review are complete.
Controller checks passed on reviewed revision `6c7a59c`, and all five source heads
were unchanged at 2026-10-13 05:21:56 UTC. Formal/docs scope is complete; runtime
findings remain open and this is not runtime or production approval.

## Global Constraints

- Review heads: PR1 d555ae0, PR2 f60a1ba, PR3 1d8adb7, PR4 e783dbf, PR5 9ec0386; refresh before final handoff.
- No runtime implementation changes, existing-PR updates, pushes, merge or deployment.
- No new runtime dependencies; formal-tool dependencies remain under `formal/quint/`.
- Preserve original user worktrees and review worktrees. Model branch: `feat/async-quint-model`.
- Tests must distinguish intended semantics from implementation correspondence; never repair a known discrepancy silently in the model.
- Exact integer pricing uses 18-decimal atomics, truncates division in order, and saturates at `2^128 - 1`; smaller state domains are explicitly labeled abstractions.
- Publish bounds, backend, seeds, samples/depth and command outcomes; simulation is not exhaustive verification or an implementation proof.
- Contract forwarding, SDK transaction atomicity, VM metering and IBC-core authentication are explicit trust assumptions.

## Review Focus

1. Zero-price polls still need an in-flight identity independent of their monetary reservation (Task 2).
2. Visit-budget contention may prevent progress without saturating gas or increasing price (Task 2).
3. Failed polls and late timer maintenance must retain previously accepted results (Task 2).
4. Authentic callback data can still be stale, duplicated, mis-correlated or too late (Task 3).
5. Restoring only task rows while omitting queue/accounting metadata is not an equivalent restart (Task 2).

---

### Task 1: Tooling and exact pricing model

**Files:** `formal/quint/package.json`, `package-lock.json`, `.gitignore`, `pricing.qnt`, `pricing_tests.qnt`, initial `README.md`.

**Interfaces:** Module `pricing`; pure integer operators `nextPrice(base, used, target, floor, denominator)` and `attemptCharge(gas, price)`. Atomics scale is `10^18`; maximum atomics is `2^128-1`.

- [x] Pin local `@informalsystems/quint` 0.33.0; verify actual version and CLI help. Do not alter global installations. If Node compatibility requires another version, pin it locally and document invocation.
- [x] Write fixture tests from `fixtures/native-async/v1/base-price.json` before functions; each fixture must assert its exact expected number. Add fee ceiling tests for zero, one atomic, one whole unit and maximum width. Capture an expected missing-function/typecheck failure before implementation.
- [x] Implement ordered wide integer arithmetic; guard invalid divisors through specified preconditions. Saturate increases at u128 and clamp decreases at floor.
- [x] Run parse/typecheck and deterministic Quint tests. Check a one-atomic deliberately wrong expected value fails, then restore the correct test; retain command/output evidence.
- [x] Document tools and exact math versus bounded scheduler abstractions. Commit this deliverable.

### Task 2: Native scheduler and restore model

**Files:** `formal/quint/native_async.qnt`, `native_async_tests.qnt`, `native_profiles.qnt`, `README.md`.

**Interfaces:** Modules `native_async`, `native_async_tests`, and finite profile modules. Export `init`, `step`, `safety`, plus named component invariants for conservation, authority, polling, rollback, price and restore equivalence. Consume Task 1 pricing helpers where exact math is required; small exploration profiles may supply explicit scaled economics.

- [x] Write deterministic scenarios before transition implementations: third-party funding does not resume/control; zero-price in-flight management is rejected by intended semantics; accepted inbox survives failed poll and expiry-independent deadline; no duplicate-height poll; matured wait -> price pause -> explicit resume retains resolution; corrupt restore is rejected.
- [x] Implement finite maps of tasks and explicit block phases. Separate poll start/reservation from callback/commit/failure so in-flight reentrancy is modeled. Include fixed-price PR4 and bid-ordered auction PR5 profiles; PR4 selection must reflect its own queue policy, not pretend auction already exists.
- [x] Include registration/funding/control/lease updates, wait delivery/timer, bounded maintenance/selection, charging, pause/resume, terminal settlement and restart. State exactly which low-level indexes/refund fallback behaviors are abstracted.
- [x] Check conservation against cumulative deposits, escrow/reservations, fees and successful refunds. Check identity/generation, gas/visit bounds, block-start price, bid eligibility, expiry, pause and preserved state. No universal liveness invariant.
- [x] Add explicit visit-cap-contention trace showing the allowed no-progress outcome with spare gas; add a clearly labeled implementation-discrepancy variant only if source evidence supports it. Never include such a variant in the normal safety proof silently.
- [x] Run typecheck/tests, seeded simulation and one tractable finite/bounded verification profile. Exercise negative controls for omitted in-flight guard and missing restore metadata. Save exact results, counterexamples and limits. Commit.

### Task 3: Delayed services, IBC and VM abstraction

**Files:** `formal/quint/service_waits.qnt`, `service_waits_tests.qnt`, `ibc_waits.qnt`, `ibc_waits_tests.qnt`, `README.md`.

**Interfaces:** Each model exports `init`, `step`, `safety`, named correlation/authorization/retained-state properties and deterministic trace tests. These models do not depend on native scheduler being merged into PR1/3.

- [x] Write scenario tests for wrong sender, stale sequence, duplicated response, local deadline, error result, callback rollback/retry and driver restart. IBC additionally tests wrong channel/endpoints/data, timeout and closure.
- [x] Implement caller/service durable state with driver-local state distinguished from chain state; success, application failure and transport rollback are different transitions. Model remote effects not reversed by local expiry.
- [x] Implement IBC packet-correlated state with authenticated host callbacks as a declared environment assumption. Do not invent packet-sequence checks or token escrow absent from the contract.
- [x] Tie abstract guest commit/rollback outcomes to PR2 test evidence and document unproved artifact/compiler/metering obligations. No Wasm interpreter in Quint.
- [x] Run parse/typecheck, deterministic tests and seeded simulations; negative controls must demonstrate callback-correlation properties detect a weakened check. Record exact scope and commands; commit.

### Task 4: Unified runner, coverage and review report

**Files:** `formal/quint/verify.sh`, `formal/quint/README.md`, `formal/quint/verification-results.md`, `docs/reviews/2026-10-12-all-open-pr-async-review.md`.

**Interfaces:** `bash formal/quint/verify.sh` runs the documented portable checks with nonzero status on any failed obligation; an explicit extended mode runs available formal backend checks. No false success when a backend is unavailable.

- [x] Write runner error-path tests/checks: missing CLI, invalid model or failed invariant must fail; optional backend skip must be labeled, not reported as verified.
- [x] Implement runner and execute on the final models. Publish actual tests/samples/depth/seed, bounds, versions and proof limits. Include at least one checked negative control for each model family.
- [x] Assemble per-PR coverage matrix including exact heads, modeled transitions, source evidence, tests and outstanding unmodeled obligations. Do not infer merge compatibility between unrelated PR branches.
- [x] Consolidate adversarial review findings with severity, lines, minimal trigger, impact, reproduction status and remediation. Separate confirmed issues, policy risks, incomplete checks and disproven hypotheses.
- [x] Refresh open PR heads; flag drift instead of silently applying old conclusions to new code. Independently review model/source correspondence and runner behavior, fix only model/report defects, rerun affected checks and commit.

## Execution recommendation

Use subagent-driven execution: fresh implementer and scoped reviewer for each task,
then one model/source-correspondence review. Native source review remains read-only
and can run concurrently. Implementation begins after this plan is approved.
