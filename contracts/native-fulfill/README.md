# Generated native fulfillment example

This experimental contract requires the native async host. `Start` registers the
generated `fulfill` workflow; payment and shipment are distinct typed operation
waits. Only privileged `sudo` accepts `TaskSudoMsg::PollTask`. Public `Resume`
forwards its actual sender and external success/error into `RuntimeMsg::Deliver`;
the host authenticates the resolver and owning contract/task pair.

Public enums retain the original example's externally tagged Rust names (`Start`,
`Resume`, `Async`, `PaymentSeen`); native host wire enums use snake case.

`InstantiateMsg` configures payment/shipment service addresses, a positive
`deadline_blocks`, and `fee_denom`. `Start` supplies `order`, `business`, `gas`,
`gas_per_poll`, `max_gas_price`, `expires_at`, and `allocated_bytes`. Attached coins
must exactly equal business plus gas, including when they share a denomination.
Business funds remain in contract custody; only the specified gas amount enters
host escrow. This example does not implement business-fund withdrawal.

Registration uses a successful submessage reply to propagate the actual raw JSON
host task ID. Its reply ID is only an internal routing constant.

`Async(FundTask|ResumeTask|CancelTask)` implements the standard funding interface.
`Async(FundingConfig|TaskFunding)` queries authoritative host funding data;
`Task` forwards the host execution-state query without lossy reserialization.
Top-up forwards actual coins and contributor, while management forwards the actual
actor. Sudo workflow contexts support ordinary CosmWasm queries; custom native
queries are available through public handlers, not the legacy `WorkflowCtx`.

Application errors become encoded `PollOutcome::Fail` and commit preceding
segment writes. Runtime/decoder errors propagate as contract errors so the host
rolls back the poll. Direct MockStorage calls do not model transaction rollback.

The optional runtime bridge also supplies `native::SleepBlocks`: a positive block
count used with `.wait::<SleepBlocks>(blocks).checkpoint(...).await?`. It reserves
`native/sleep_blocks/v1`, emits no messages, and maps to host `Wait::Height`.
Only `Resolution::Timer` resumes this adapter. The bridge rejects time deadlines
and malformed metadata or messages attached to that reserved identity. A generated
timer workflow is tested alongside the generated two-operation workflow.

## Wasm artifacts

Run `bash contracts/native-fulfill/build-wasmd.sh`. The script builds locked Cargo
dependencies and normalizes sign extension, bulk memory, and saturating float
conversions for wasmd v0.55.1 / wasmvm 2.2.4 using Binaryen 123. On arm64 macOS it
downloads a checksum-pinned release; elsewhere set `WASM_OPT` to Binaryen 123.
It creates `artifacts/native_fulfill_example.wasm` and `artifacts/native_service.wasm`.
The current build used rustc 1.99.0; normalization does not by itself prove VM
compatibility. Actual VM validation belongs to the pinned wasmd harness.

`native_service` is an explicit harness fixture, not a production fulfillment
service. Instantiate `{}` records its operator. `Begin` stores correlation and
the actual requesting contract. The operator calls `Release { correlation,
outcome }` to emit that contract's `Resume`. Deploy separate payment and shipment
instances. The fixture supports a single owning workflow contract per service;
it has no production admission, accounting, or multi-tenant policy.
