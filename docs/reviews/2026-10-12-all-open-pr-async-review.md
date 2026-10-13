# All-open-PR durable async review — 2026-10-12

Review covers each of the five open PRs at its own pinned head, with executable
Quint abstractions and separate implementation evidence. N1 and N2 are
source-confirmed Rust/Go discrepancies; neither has a new runtime reproduction
in this review. R1 is an economic-policy/liveness risk. Two lower-priority
demonstration/documentation issues are recorded below. No runtime fix, external
PR comment, push, merge, deployment or signing was performed. Final independent
whole-branch model/source/runner review is pending; scoped model tasks passed review.

## Inventory and coverage

Refreshed again at **2026-10-13 04:59 UTC** (2026-10-12 local) with:

```sh
gh pr list --repo iqlusioninc/cosmwasm-durable-async --state open --json number,title,headRefOid,headRefName,baseRefName,url
```

Exactly PRs 1–5 were open; every head still matched the reviewed source. A future
head change invalidates automatic attribution of these conclusions to new code.

| PR / exact head | Modeled transitions and source anchors | Separate implementation evidence | Outstanding obligations |
| --- | --- | --- | --- |
| [#1 delayed services](https://github.com/iqlusioninc/cosmwasm-durable-async/pull/1), `d555ae00919a01050770f47fbbd0dd498a33481e`; base `main` | `service_waits`: immutable service binding, Begin correlation tombstones, Deliver/Expire/Prune, application failure versus abort, per-service FIFO, journal Prepare/Broadcast/Reconcile/Restart. [Service](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/d555ae00919a01050770f47fbbd0dd498a33481e/contracts/demo-service/src/lib.rs#L81-L150), [inherited runtime](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/d555ae00919a01050770f47fbbd0dd498a33481e/crates/durable-runtime/src/runtime.rs#L245-L282), [driver](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/d555ae00919a01050770f47fbbd0dd498a33481e/scripts/service-driver.py#L13-L102) | 31 Rust integration-test executions, including deliberately repeated baseline scenarios and 4 unique new adversarial scenarios; 7 mocked Python driver tests. Model: 16 deterministic cases, 500×80 simulation, sequence mutation control. | Real signer/broadcast/receipt execution, OS/power-loss journal faults, SDK/VM rollback, fees and external-effect truth/idempotency. No native scheduler or auction applies here. |
| [#2 VM validation](https://github.com/iqlusioninc/cosmwasm-durable-async/pull/2), `f60a1ba85b94d221870265f7ecaca0405dc0d552`; base `main` | VM commit/abort is an **assumption** in the models, with named obligations mapped to [five compiled tests](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/f60a1ba85b94d221870265f7ecaca0405dc0d552/tools/vm-tests/tests/compiled.rs). [Build/check scripts](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/f60a1ba85b94d221870265f7ecaca0405dc0d552/scripts/check-wasm.sh) pin/build/normalize/hash the artifact. | Current normalized fulfill artifact rebuilt and five compiled VM tests passed: fresh invocations, authority/stale callback, deadline/domain failure, OOG, payload cost. | No Wasm interpreter/compiler/ABI equivalence theorem, SDK atomicity or outbound dispatch proof. OOG fixture discards failed backend; payload sweep is not host gas/size calibration. |
| [#3 IBC waits](https://github.com/iqlusioninc/cosmwasm-durable-async/pull/3), `1d8adb79847ddd764693fba891a64450d70cd6ca`; base `feat/wasm-vm-validation` | `ibc_waits`: immutable bind/close, exact intent data/endpoints/timeout, authenticated host commitment outcomes, self Deliver, two waits, local deadline, early remote timeout, atomic abort, restart. [Adapter](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/1d8adb79847ddd764693fba891a64450d70cd6ca/contracts/ibc-query/src/lib.rs#L274-L463), [host assumptions](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/1d8adb79847ddd764693fba891a64450d70cd6ca/docs/ibc-example.md#L88-L139) | 10 native contract + 6 focused public-API tests + 2 compiled IBC VM tests. Model: 14 deterministic cases, 500×80 simulation, host/data mutation controls. | Real IBC proof/core replay handling and SDK commitment rollback, live node restart/state sync, arbitrary encodings/width overflow. Sequence authority belongs to IBC core, not an adapter check. Current artifact differs from historical chain-smoke artifact. |
| [#4 native runtime](https://github.com/iqlusioninc/cosmwasm-durable-async/pull/4), `e783dbf83152842cfb211eda7554caaf3afd861a`; base `main` | `native_async` fixed-price profile: registration/funding/policy/lease, waits/results, ring/FIFO scheduling, bounded visits/maintenance, explicit flight, commit/rollback/charging, settlement and durable projection/restore. [Rust poll loop](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/e783dbf83152842cfb211eda7554caaf3afd861a/crates/native-async/src/runtime.rs), [Go scheduler](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/e783dbf83152842cfb211eda7554caaf3afd861a/integration/wasmd/scheduler.go#L116-L136) | Own-head all-features Rust: 107 test/harness cases pass, none failed/ignored. Native model's 29 cases shared with PR5; fixed simulation 500×80. | N2 deadline/resolver parity; expired-head ordering differs. Byte/schema/code/apphash activation validation, refund fallback, admission/byte caps, full restore bisimulation and host calibration are not proved. No dedicated own-head Go rerun. |
| [#5 auction](https://github.com/iqlusioninc/cosmwasm-durable-async/pull/5), `9ec038632388343738c72b60e5512d29d2669e66`; base `feat/native-async-funding` | `pricing` exact integer fixtures; `native_async` bid/FIFO, scan-past oversized work, preserved grace age, block-start price, consumed-gas charge and next-price update. [Pricing](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/9ec038632388343738c72b60e5512d29d2669e66/crates/native-async/src/pricing.rs), [selection](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/9ec038632388343738c72b60e5512d29d2669e66/integration/wasmd/scheduler.go#L228-L241) | 147 all-features Rust test/harness cases pass; Go upstream package including opt-in local three-process state-sync test passes. Model: 28 pricing + shared 29 native cases, auction 1,000×80, flight/restore mutation controls. Prior accounting boundary: Apalache depth 1. | N1 zero-price flight exclusion; R1 visit economics; N2 inherited. Same native abstraction limits as PR4, plus small-price rounding differs from production Decimal economy. No universal priority fairness or economic-resistance theorem. |

PR2→PR3 and PR4→PR5 are existing stacks. PR1 is independent. The model branch is
based on PR5; PR1/PR3 contract paths are absent here and are linked at their own
heads. No merge compatibility between these branches was tested or inferred.
The 87 deterministic cases and simulations are formal-model evidence, not counts
of source implementations proved correct. [Verification results](../../formal/quint/verification-results.md)
record seeds, bounds, witnesses, exact backend outcomes and deferred tool warnings.

## Source-confirmed findings

### N1 — P1 Rust reference runtime: zero-price poll loses in-flight exclusion

Introduced by PR5 permitting zero price atop PR4's reservation-based guard.
[`runtime.rs:451–453`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/9ec038632388343738c72b60e5512d29d2669e66/crates/native-async/src/runtime.rs#L451-L453)
and [801–803](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/9ec038632388343738c72b60e5512d29d2669e66/crates/native-async/src/runtime.rs#L801-L803)
guard management using nonzero monetary reservations. Reservation at
[1216–1218](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/9ec038632388343738c72b60e5512d29d2669e66/crates/native-async/src/runtime.rs#L1216-L1218)
is price-derived; zero price yields zero while polling. Outbound dispatch at
[1253–1279](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/9ec038632388343738c72b60e5512d29d2669e66/crates/native-async/src/runtime.rs#L1253-L1279)
admits callbacks through the owning contract and reloads the task afterward.

Minimal trigger: funded/leased task at supported zero base price → reserve/start
poll (money reserve 0) → authorized same-task controller cancels during outbound
dispatch → host resumes settlement. No cross-contract authority forgery is needed.
For Complete/Fail, already-terminal rejection in `finish_with` (900–901) and
fallbacks makes `run_block` return an error before committing the block cache
(1010–1018, 1319–1323). For Yield, 1309–1314 rewrites funding status to Runnable
on the Cancelled execution task. These are source-derived consequences.

Go uses an independent polling flag at
[`scheduler.go:505`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/9ec038632388343738c72b60e5512d29d2669e66/integration/wasmd/scheduler.go#L505),
[`keeper.go:386–387`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/9ec038632388343738c72b60e5512d29d2669e66/integration/wasmd/keeper.go#L386-L387)
and [`management.go:23–27`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/9ec038632388343738c72b60e5512d29d2669e66/integration/wasmd/management.go#L23-L27).
PR4's [`host.rs:243–246`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/e783dbf83152842cfb211eda7554caaf3afd861a/crates/native-async/src/host.rs#L243-L246)
rejects zero price; attribution therefore requires PR5's configuration change.

Reproduction status: source-confirmed and model mutation reproduced; **no new
runtime end-to-end reproduction**, Go/wasmd chain halt or deployed consensus
fault established. Intended model always uses explicit flight identity;
`weakFlightInit/weakFlightStep` demonstrates the omitted guard. Remedy: an
independent in-flight identity applied to all management operations, with zero-price
callback regressions covering terminal and nonterminal outcomes. Runtime unchanged.

### N2 — P2 Rust/Go pending-wait acceptance diverges at lease expiry

Introduced in PR4 and present in PR5. Rust checks lower due bound and size at
[`runtime.rs:973–974`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/9ec038632388343738c72b60e5512d29d2669e66/crates/native-async/src/runtime.rs#L973-L974),
but Go additionally requires `due < expires_at` at
[`scheduler.go:413–414`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/9ec038632388343738c72b60e5512d29d2669e66/integration/wasmd/scheduler.go#L413-L414).
PR4 own-head equivalents are Rust
[912–919](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/e783dbf83152842cfb211eda7554caaf3afd861a/crates/native-async/src/runtime.rs#L912-L919)
and Go [263–268](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/e783dbf83152842cfb211eda7554caaf3afd861a/integration/wasmd/scheduler.go#L263-L268).

Minimal trigger: task expiry H=10, poll H=2, return timer due=10 or operation
deadline=10 (also >10). Rust accepts continuation/outbound messages then later
expires the task; Go rejects the response, rolls back its effects and pauses
`invalid_response`. Impact includes committed application effects and status/result
semantics. Related policy mismatch: Rust resolver is nonempty and ≤256 bytes
(967–968); Go requires Bech32 (409–410). Address validity is abstracted in Quint.

Reproduction status: **source-confirmed, no new runtime differential reproduction**.
`expiryBoundaryWaitRejectedTest` checks intended Go-aligned rejection and guest
rollback; it does not prove Rust already rejects. Remedy: align acceptance policy
and add differential cases for due==expiry, due>expiry and invalid resolver.
No presently deployed mixed-host validator configuration is claimed.

### L1 — Low local-demo availability: HTTP polls have no timeout

PR1 [`local-service-demo.py:67,104–108`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/d555ae00919a01050770f47fbbd0dd498a33481e/scripts/local-service-demo.py#L67)
calls `urllib.request.urlopen` without a timeout; cleanup is 113–117. A loopback
listener that accepts but never answers stalls one request indefinitely, so the
60-attempt startup bound cannot advance and cleanup waits for interruption. No
adversarial chain account is required or sufficient. Source-confirmed; no hung-node
socket experiment was performed. Inferred impact is a hung local demo/retained
child node, with no fund-loss or authorization bypass shown. Remedy: explicit
per-request timeouts and overall monotonic startup/expiry deadlines. The separate
driver already supplies `timeout=10` at `service-driver.py:75`.

### D1 — Optional P3 documentation: nested correlation accepts unknown fields

PR3 [`docs/ibc-example.md:88`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/1d8adb79847ddd764693fba891a64450d70cd6ca/docs/ibc-example.md#L88)
says schemas reject unknown fields. Top-level
[`Packet` (54–59)](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/1d8adb79847ddd764693fba891a64450d70cd6ca/contracts/ibc-query/src/lib.rs#L54-L59)
does, but inherited
[`Correlation` (45–49)](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/1d8adb79847ddd764693fba891a64450d70cd6ca/crates/durable-runtime/src/types.rs#L45-L49)
does not. Minimal trigger:
`{"correlation":{"workflow_id":1,"wait_sequence":1,"extra":7},"value":5}`
decodes as Packet. Reproduced by the isolated PR3 public-API test (6/6 passing).
No exploit shown: callbacks compare exact saved bytes, and public Deliver is
self-sender gated. The overly broad claim is added by PR3; permissive shared type
is inherited. Remedy: qualify the guide or assess compatibility before making
shared correlation strict. JSON schema strictness is outside the Quint abstraction.

## Economic-policy risk and correspondence qualification

R1: PR5's strict bid ordering pays the base price, while every inspection consumes
`MaxVisits` ([`scheduler.go:228–241`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/9ec038632388343738c72b60e5512d29d2669e66/integration/wasmd/scheduler.go#L228-L241)).
Persistent higher-bid yield tasks can exhaust visits with spare gas, repeatedly
excluding lower bids while execution-only utilization stays below target. The
bounded `visitCapContentionTest` demonstrates two high bids taking both visits for
two blocks, a lower bid never polling and one gas unit unused per block. This is
an allowed priority outcome, **not a violated universal fairness promise**.
Admission/storage and actual poll gas still cost money; no numeric attack-cost or
free-service claim is established. Default per-owner 20 slots/default 20 visits
suggest concentration is possible, but real maintenance/host gas needs measurement.
Policy options: price inspections/host work, reserve a fair lane, or tie admission
to this scarce resource. Choosing a policy is outside this formal/report change.

PR4 expired-head ordering: Rust
[`runtime.rs:1054–1069`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/e783dbf83152842cfb211eda7554caaf3afd861a/crates/native-async/src/runtime.rs#L1054-L1069)
settles expiry before gas-fit/ring rotation; Go
[`scheduler.go:116–136`](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/e783dbf83152842cfb211eda7554caaf3afd861a/integration/wasmd/scheduler.go#L116-L136)
checks gas fit and rotates first. An expired head plus live same-owner sibling
and another owner can change the next inspection/refund timing under bounded
visits. The fixed model follows Go at this boundary; exact parity for both hosts
is not claimed. This is source-confirmed correspondence qualification with no
dedicated differential execution or newly assigned high-severity vulnerability.

## Implementation test and artifact receipts

These review runs preceded Task 4; no heavy runtime suite was repeated for the
runner/report change. Temporary repro checks remain outside production source.
Their essential commands/results are committed here instead of relying only on
ignored reports or temporary files.

| Head / execution | Actual command and result |
| --- | --- |
| PR1 isolated identical-head service repro | `cargo test --locked -p demo-service -p fulfill-example -p durable-runtime`: 31 integration-test executions, including 3 intentionally rerun baseline cases via include and 4 unique added adversarial scenarios; no failures. `python3 -m unittest discover -s scripts -p 'test*driver*.py'`: 7 pass (4 added). cw-multi-test/mocked CLI, no real signing/SDK fees. |
| PR2 normalized build/check | From `/private/tmp/cosmwasm-review-pr2.LDAo7r`: `WASM_OPT=/private/tmp/binaryen-version_123/bin/wasm-opt bash scripts/check-wasm.sh` rebuilt normalized artifact. Initial VM stage failed only on unwritable default Cargo cache. Retry: `CARGO_HOME=/private/tmp/native-async-vm.XULPGP/cargo DURABLE_WASM=/private/tmp/cosmwasm-review-pr2.LDAo7r/target/artifacts/fulfill_example.wasm cargo +1.85.1 test --manifest-path tools/vm-tests/Cargo.toml --locked -- --nocapture`: exit 0, all 5 compiled VM tests pass, none ignored. |
| PR3 native/focused contracts | `cargo test -p ibc-query-example --locked --target-dir /private/tmp/cosmwasm-adversarial.LDAo7r/ibc-target`: 10 pass. `cargo test --manifest-path /private/tmp/cosmwasm-adversarial.LDAo7r/ibc-repro/Cargo.toml --target-dir /private/tmp/cosmwasm-adversarial.LDAo7r/ibc-target`: 6 pass. |
| PR3 rebuilt artifacts and IBC VM | From `/private/tmp/cosmwasm-review-pr3.LDAo7r`: `WASM_OPT=/private/tmp/binaryen-version_123/bin/wasm-opt CARGO_HOME=/private/tmp/native-async-vm.XULPGP/cargo scripts/build-wasm.sh fulfill-example ibc-query-example`: exit 0, Rust 1.99.0/Binaryen 123. Then `CARGO_HOME=/private/tmp/native-async-vm.XULPGP/cargo IBC_WASM=/private/tmp/cosmwasm-review-pr3.LDAo7r/target/artifacts/ibc_query_example.wasm DURABLE_WASM=/private/tmp/cosmwasm-review-pr3.LDAo7r/target/artifacts/fulfill_example.wasm cargo +1.85.1 test --manifest-path tools/vm-tests/Cargo.toml --locked --test ibc -- --nocapture`: 2 pass; separate five-test `compiled.rs` target not run at PR3. |
| PR4 own head | From `/private/tmp/cosmwasm-review-pr4.rlWLZc`: `CARGO_TARGET_DIR=/private/tmp/cosmwasm-native-async.i7sHwm/target cargo test --workspace --all-features --locked`: exit 0, 107 Rust test/harness cases pass, none failed/ignored. |
| PR5 formal-only branch baseline | `cargo test --workspace --all-features`: exit 0, 147 Rust test/harness cases pass, none failed/ignored; includes compile fixtures and feature-gated native tests. |
| PR5 Go/local state sync | From `/private/tmp/cosmwasm-auction.pQVLta/integration/wasmd`: `GOTOOLCHAIN=go1.23.6 GOPATH=/private/tmp/native-async-go.G2oH2c GOCACHE=/private/tmp/native-async-go.G2oH2c/build GOWORK=/private/tmp/native-async-workspace.pSsElG/go.work NATIVE_ASYNC_NETWORK_TEST=1 go test -tags upstream -count=1 -timeout 5m ./...`: exit 0, package 25.404s; two command packages have no tests. Uses existing patched local wasmd/VM checkout, no fresh native-library rebuild. Opt-in local three-process state-sync test enabled; no public-network/Byzantine consensus proof. |

| Artifact | SHA-256 |
| --- | --- |
| PR2 rebuilt normalized fulfill | `c01e95b82e68444a16b462ac75854f585c4e0000be38105707b8ec53b01e10a7` |
| PR3 rebuilt normalized fulfill | `4076439bf9f48b89f21ea1514b143e89a717fb90bff6c80d5c2d93f64fd66df2` |
| PR3 rebuilt normalized IBC | `1a1bd09bd659cda39560870e2add008942dcfafa51f066c2a1e091d83ded1dd9` |
| PR3 historical [chain-smoke IBC artifact](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/1d8adb79847ddd764693fba891a64450d70cd6ca/docs/ibc-smoke-result.json) | `e4bea19387c8971e3d7484f9c589d7c72a2039994d683481f872a86aa4c55116` |

Compiled IBC fixtures validate capabilities, execute all six classic exports and
relay application bytes across two VMs, but manually dispatch self messages.
They do not test real IBC proofs/SDK rollback or live node restart. The historical
smoke artifact differs; no historical chain result is attributed to the current
rebuilt hash, and no fresh chain deployment occurred in this review.

## Rejected hypotheses, incomplete checks and residual assumptions

- Driver journal truncation: source writes/fsyncs a sibling temporary file,
  atomically replaces the journal, then fsyncs its parent (PR1 driver 13–22,
  93–101). An in-place hash-save truncation window is unsupported. Persistence
  assumptions remain; no OS/power-loss fault injection proves arbitrary filesystems.
- Forged snapshot activation: inspected PR5 `snapshot.go`/`snapshot_state.go`
  require pending activation marker, trusted apphash/height, schema/config manifest,
  complete bytecode hashes, canonical index/task agreement and aggregate backing.
  Failed restore remains fail-closed. Missing price keys use explicit fallback;
  operator corruption is not a demonstrated peer forgery preserving apphash.
- Admission-equivalent restore restrictions (per-task bytes versus HostReserve,
  per-owner active cap) are defense-in-depth validation gaps without a demonstrated
  reachable valid-state/authenticated snapshot attack. The model does not cover them.
- Apparently unbounded failure-recording work is bounded by registration/lease
  allocation `(HostReserve-1_000_000)/120`; 120 remains a calibration obligation.
- IBC contract-side packet-sequence omission is an explicit host boundary. Direct
  mock invocation with sequence 987654 advances; a conforming IBC core authenticates
  commitment and sequence. No forged on-chain callback exploit was established.
- Authenticated early remote timeout legitimately leaves Waiting without pending
  until local Expire is submitted. Deadline passage alone does not resolve service
  or IBC workflows. Neither is a lost-wait bug under the documented external-driver model.
- Empty payment reference commits `PaymentSeen` plus terminal failure; empty
  shipment tracking succeeds; order IDs may repeat. These inherited/demo semantics
  are modeled as observed, not promoted to payment/shipping guarantees. Contracts
  reject attached funds and implement no business escrow/refunds or real payment proof.
- Compiler/native bridge lowering was sampled; no concrete defect found, but
  exhaustive branch/loop/capture/child-helper equivalence and native memory safety
  were not reviewed/proved. Guest versus infrastructure failure provenance and
  start-section/ABI/cache admission were source-inspected; no deterministic guest
  input was demonstrated to trigger an infrastructure panic.
- Four independent native review assignments (economics, lifecycle, restore and
  VM boundary) were interrupted by platform cybersecurity filtering before final
  reports. They were **not retried by rephrasing**, and no completed independent
  coverage/runtime reproduction is attributed to them. Controller source inspection,
  scoped model reviews and existing tests provide narrower evidence. This is a
  material independent-review depth limitation.
- Formal exclusions include cross-task callback/reentrant transaction rollback,
  failed-refund collector fallback, byte-level queues/cursors/encoding, separately
  delegated controllers, host gas calibration, serialization/apphash authentication,
  actual Wasm meter/compiler correctness and complete restart bisimulation. No
  unconditional liveness/fairness, external exactly-once effects or production
  readiness conclusion follows from finite/seeded model checks.

The committed [results](../../formal/quint/verification-results.md) retain the
successful restricted depth-one Apalache accounting check, aborted broader search,
TLC width failure and unresolved npm/Apalache development dependency warnings.
Those limitations and source findings remain open; this change documents them
and supplies reproducible formal checks, without repairing runtime behavior.
