# Native async examples: DCA first

Date: 2026-10-15

Status: proposed written design; the example direction and DCA-first ordering
are approved, but this document and implementation plan still require review.

## Purpose and scope

Build convincing, runnable examples for contract developers evaluating native
CosmWasm async. The examples must make durable execution, automatic wakeups,
funding, and recovery observable, with tutorial-quality explanations of their
limits. They are local test-token demonstrations, not production financial
contracts or evidence that the runtime is production-ready.

The agreed suite is:

| Example | Distinct capability | Delivery order |
| --- | --- | --- |
| Scheduled DCA trades | Timers, durable loops, atomic external effects, gas recovery | First implementation |
| Milestone escrow | Authenticated approvals, deadlines, explicit dispute/refund policy | Separate follow-up design |
| Sealed-bid auction | Timed phases, delayed scheduling, deterministic settlement | Separate follow-up design |

This document specifies only the first implementation. It does not silently
choose escrow dispute rules or auction economics. Success for DCA is a finite
order that trades across blocks, survives process restart, pauses when its gas
budget is insufficient, resumes only when authorized, and never repeats an
already committed slice.

## Approach and current baseline

Use generated durable workflows on the native scheduler, with a small local
fixed-price exchange fixture. This exercises the developer-facing async syntax
and the real scheduler without introducing oracle, DEX, or cross-chain trust.

Two alternatives are deliberately deferred: a hand-written native poll state
machine would demonstrate the ABI but obscure the async authoring experience;
a real DEX/IBC integration would demonstrate more realistic markets but add
adapter and reconciliation work unrelated to the first example. The mock is
not an AMM, live market, or price oracle.

The implementation baseline is PR #5, commit
`9ec038632388343738c72b60e5512d29d2669e66`, which includes PR #4's native runtime.
PR #6's [adversarial review](https://github.com/iqlusioninc/cosmwasm-durable-async/blob/0619cc691e2df77a9ee8ca117415aec10544a4b0/docs/reviews/2026-10-12-all-open-pr-async-review.md)
is relevant evidence, not a runtime repair. Revalidate heads before implementing.

Existing interfaces already provide generated loops, `SleepBlocks`, operation
waits, privileged polls, native funding, and ordinary nested contract messages.
The generated bridge emits messages from pending operations, not terminal
returns; `WorkflowCtx` cannot make native custom queries. Consequently the
design uses an operation adapter plus ordinary execute/reply handlers for money
movement. No compiler, scheduler ABI, or native IBC extension is proposed.

## Components

- `contracts/native-dca`: order admission, generated workflow, swap adapter,
  authenticated settlement, funding/control routing, and queries.
- `contracts/mock-exchange`: explicitly labeled fixture with immutable native
  input/output denominations and a positive fixed integer exchange ratio.
  Output liquidity is supplied with local test coins. It has no public-market
  price discovery, fees, or production administration mechanism.
- Contract tests: transactional host simulation, including failure injection,
  reconstruction from stored state, and two simultaneous orders.
- `integration/wasmd`: new scenarios using built artifacts in the existing
  patched chain harness; no broad harness rewrite.
- Tutorial and local scenario runner: reproducible setup and recovery receipts,
  with state/funding queries after each important transition.

Keep protocol types and accounting helpers small and local to these examples.
Do not extract a general-purpose exchange framework before another real user
of that abstraction exists.

## Order terms, storage, and public interface

`CreateOrder` accepts input amount per slice, slice count, minimum output per
slice, block interval, fixed beneficiary, initial gas coin, gas-per-poll cap,
maximum gas price, immutable expiry height, and allocated bytes. The exchange
address and fee denomination are deployment configuration, not arbitrary
per-swap caller overrides. Validate configured fee denomination against the
host and exchange denominations against its immutable configuration.

Use native coins only, distinct input/output denominations, positive slice
amount/minimum output/interval, and a slice count in `1..=128`. Compute total
principal with checked arithmetic. Attached funds must exactly equal total
principal plus initial gas, coalescing denominations with checked addition when
they coincide. Unexpected, duplicate, zero, or excess coin entries are rejected.
Only the gas allocation is forwarded to host escrow.

Each order stores immutable owner, beneficiary, exchange, terms, expiry and
registration height; its host task ID; remaining and reclaimed principal;
filled count; cumulative input spent/output delivered; last fill height; and any pending
dispatch. The owner comes from the actual `MessageInfo.sender`, never the
synthetic self-sender used during native polling.

Allocate a monotonic contract-local order ID and register `native_dca/v1` with
that ID as workflow input. A successful registration reply binds the actual
host task ID and returns both IDs to the caller. Registration failure rolls
back order creation and incoming funds. Reply payloads correlate registration
without a shared process-local variable.

Bound successful result payloads to 2048 bytes and preflight the allocation
against host metadata, the SDK's worst-case inbox reserve, and the bounded
serialized continuation. The example retains order/fill records, at most 128
fills per order; automatic pruning of application history is not included and
must not be confused with the host's leased task retention.

Expose:

- `Async(FundTask | ResumeTask | CancelTask)` and
  `Async(FundingConfig | TaskFunding)`, preserving actual actors/contributors.
- `UpdateBudget` for the owner to change the poll cap or price ceiling. Keep
  gas refund ownership fixed to that owner; updates do not resume a task.
- `ReclaimPrincipal { order_id }`, owner-only, after host termination or the
  immutable expiry boundary. It returns only that order's remaining principal.
- `Order`, `Fill`, and `Task` queries. `Order` includes application accounting
  and separate host status when available; it must represent a pruned host
  record explicitly, not turn a query failure into a completion claim.

No lease extension, business-budget top-up, beneficiary change, order editing,
public manual pause, or active-task migration is included. Fixed expiry makes
post-pruning principal recovery unambiguous. Funding a task does not grant
control, change recipients, or automatically resume it. A plain bank transfer
is neither order principal nor scheduler credit.

## Schedule and durable workflow

The first target is registration height plus interval. Subsequent targets are
the previous committed fill height plus interval. These are minimum eligible
heights, not promised execution times. The scheduler may run later because of
price, gas, maintenance limits, or congestion. Block intervals are not seconds.

Conceptually:

```text
read order progress
  -> sleep until next target if still in the future
  -> dispatch one swap and await its authenticated receipt
  -> consume receipt and read committed progress
  -> repeat, or complete
```

Never construct `SleepBlocks(0)`. If a target is already due, attempt one slice;
do not loop through missed intervals or issue a catch-up batch. The next target
uses the actual new fill height. Use explicit typed checkpoints and immutable
workflow versioning under the existing macro restrictions.

The synchronous fixture's operation deadline is dispatch height plus one.
Before constructing any wait, check overflow and require its due height to be
strictly less than immutable task expiry. If a future timer or swap deadline
cannot fit, complete with `LeaseWindowExhausted`, preserving unspent principal
for explicit reclaim. A task already expired in the host cannot poll; its order
is recoverable through the public reclaim path instead.

## Atomic swap and receipt protocol

The operation's saved resolver is the configured exchange. Its pending message
calls `DispatchSlice` on the DCA contract itself. The host stages the wait before
dispatching this message, so a same-transaction exchange callback can fill the
inbox without recursively polling the workflow.

1. `DispatchSlice` is self-only and accepts no funds. It validates order/task,
   next slice index, correlation, remaining principal, interval and lease bounds
   against stored terms. It records a bounded pending dispatch and debits the
   slice's principal before sending the exact input coin to the exchange.
2. Dispatch uses a normal execute submessage with success reply handling. This
   is inside the self-execute handler, not a `SubMsg` in `PollResponse.messages`.
3. The exchange validates the exact attached input and rejects a previously
   executed `(DCA address, order ID, slice index)`. It computes floor-rounded
   output with checked wide integer arithmetic and rejects zero, overflow,
   output below the requested minimum, or insufficient liquidity.
4. The exchange records its receipt, sends output directly to the fixed
   beneficiary, then sends `SwapResult` to the actual requesting DCA address.
   It must not accept a separately claimed callback target. The result binds
   order, slice, correlation, denominations, amounts, and beneficiary.
5. `SwapResult` requires the actual exchange sender and the exact pending
   dispatch. It validates the receipt, records the fill, updates counters and
   clears the pending dispatch, then forwards `RuntimeMsg::Deliver` with that
   actual sender. No caller-supplied resolver or replacement recipient is used.
6. The dispatch reply requires the expected fill receipt and cleared pending
   marker. A missing callback, malformed callback, bank failure, failed host
   delivery, or failed reply aborts the entire poll, including both contracts'
   writes and all transfers. Successful reply returns no further effects.

Registration and dispatch replies use distinct tags and validated payloads.
Unexpected replies fail. The real-VM tests must establish this nested
execute/callback/reply behavior; its inclusion in this design is not a claim
that the new path has already been tested.

The host consumes a successful inbox no earlier than the next block. Application
accounting already reflects the trade before that consumption. Cancellation,
expiry, or restart in this interval therefore cannot erase the fill or make its
input reclaimable. The callback receipt is not independent proof against a
malicious exchange: correctness relies on the fixed reviewed fixture and actual
transactional bank effects, which tests must assert.

## Failure, cancellation, and accounting

Min-output rejection, insufficient exchange liquidity, and failed outbound
execution are transaction failures. They roll back the slice and pause the task
according to the host's failure classification, while consumed gas is charged.
After correcting a recoverable condition, the owner explicitly resumes. The
workflow never retries trades automatically after an ambiguous timeout.

A timeout or remote-error resolution is unexpected under the mandatory atomic
success-callback protocol. Terminate with a diagnostic outcome and no replacement
swap. Do not describe local timeout or task cancellation as undoing an earlier
trade. Application `Fail` commits preceding application writes, unlike a failed
poll transaction; settlement validation must use the transaction-error path.

After cancellation, completion, failure, or expiry, reclaim transfers only
remaining principal to the stored owner, zeroing its claim atomically with the
bank send. A second reclaim cannot pay again. Before expiry, query authoritative
host status and propagate errors; at/after the immutable expiry, recovery does
not require an unpruned host record. Cancel and reclaim may be separate user
transactions. Host gas refunds remain entirely separate.

Committed-state invariants include:

- Initial principal = cumulative input spent + remaining principal + reclaimed
  principal; none of these terms includes gas escrow.
- At most one committed fill per order/slice; each fill satisfies its minimum
  output and pays the fixed beneficiary exactly once in the fixture protocol.
- Filled count never exceeds the configured count; committed pending dispatches
  do not exist when the atomic callback/reply guard succeeds.
- Sponsors cannot change the owner, beneficiary, refund address or execution
  authority; one order cannot spend or reclaim another order's principal.
- `all slices filled` is distinct from `host workflow completed`. The host may
  still be runnable, or become cancelled/expired, after the last fill.

Queries and tutorial logs must show both application and host progress instead
of collapsing these last two states.

## Validation and runnable demonstrations

Tests are requirements for implementation, not results reported by this spec.
Use transactional harnesses for rollback claims, not bare `MockStorage` calls.

1. Happy path: three trades over multiple blocks, exact bank balances, receipts,
   principal/gas separation, final host outcome and escrow refund.
2. Scheduling: exact target, late dispatch, one-slice progress, no catch-up burst,
   checked height overflow, due equal to expiry, and exhausted lease window.
3. Funding: insufficient gas pauses without a trade; sponsor top-up changes no
   authority and does not resume; owner resume continues exactly once. Include
   price-limit pause and authorized cap update with no implicit resume.
4. Atomicity: insufficient liquidity, minimum-output rejection, missing/duplicate
   receipt, invalid amount, bank failure, callback rejection and reply rejection
   roll back business effects while retaining the documented gas charge.
5. Authorization/accounting: forged self dispatch, wrong exchange, stale sequence,
   duplicate slice, unexpected funds, owner/sponsor confusion, same-denomination
   gas/principal, two orders, and repeated reclaim.
6. Lifecycle: cancel before first fill and between fills; cancel/expire after
   receipt but before consumption; recover principal after host record pruning.
7. Persistence: reconstruct from stored state before a timer and after receipt;
   in the pinned app harness, commit and close/reopen the database between fill
   and consumption. Check identical continuation, bank balances and fill count.
8. Build/VM: normalized Wasm artifacts, native custom routing, the exact nested
   reply guard, gas-metered execution, and existing workspace regression suites.

The tutorial runner provisions only local test assets, uses bounded waits and
RPC timeouts, records transaction hashes/results, and stops on ambiguous
broadcast outcomes rather than automatically resubmitting. It provides separate
happy-path, low-gas/top-up/resume, liquidity-failure, cancellation/reclaim and
restart scenarios, with clean owned-process shutdown. A simulation-only result
must not be labeled as a real-node restart or state-sync result.

New DCA state-sync-network and Quint model extensions are follow-up verification
work, not implied by the existing runtime model or by database-reopen coverage.

## Known runtime boundaries and acceptance

N1 (Rust zero-price in-flight guard), N2 (Rust/Go wait acceptance mismatch) and
R1 (visit-budget starvation policy) remain open in the reviewed baseline. This
example does not fix them. Use a strictly positive minimum base price in local
demo configuration, validated addresses, and the stricter due-before-expiry
policy. These constraints limit the demonstration, not establish general runtime
safety or a fairness guarantee. Do not hide baseline failures by weakening tests.

No real funds, public-chain deployment, PR merge, runtime repair, IBC adapter,
arbitrary Rust futures, unrestricted concurrency, or production gas calibration
is authorized by this design. If implementation requires a new runtime interface
or reveals a blocking runtime defect, report it and separate that work explicitly.

The first example is complete only when its code, documentation, local runner,
transactional tests and pinned real-VM acceptance evidence agree on the above
semantics. Record exact commits, commands and limitations. Only then use the
lessons to design milestone escrow and the sealed-bid auction.
