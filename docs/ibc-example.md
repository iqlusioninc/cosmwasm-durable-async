# Durable IBC query example

`contracts/ibc-query` runs a two-wait workflow across an IBC classic unordered
channel. Both chains deploy the same contract. The peer doubles a number and
returns an acknowledgement; the first acknowledgement resumes the workflow and
sends a second request. Starting with 5 completes with 20. No tokens move and the
remote operation has no side effects.

```rust
#[durable_workflow(kind = "double_twice", version = 1)]
pub async fn double_twice(ctx: WorkflowCtx, value: u64) -> Result<u64, WorkflowError> {
    let doubled: u64 = ctx.wait::<RemoteDouble>(Request { value })
        .checkpoint().await?;
    let result: u64 = ctx.wait::<RemoteDouble>(Request { value: doubled })
        .checkpoint().await?;
    Ok(result)
}
```

There is no future executor. Each acknowledgement invokes the next synchronous
segment. The adapter persists an intent before returning `IbcMsg::SendPacket`.
The chain assigns the IBC packet sequence; the workflow has its own independent
`(workflow_id, wait_sequence)` correlation key.

## Run the fixture tests

```sh
cargo test -p ibc-query-example --locked
scripts/install-binaryen.sh
rustup toolchain install 1.85.1 --profile minimal --component clippy
scripts/check-wasm.sh
```

`check-wasm.sh` builds and validates both contract artifacts and runs compiled
Wasm tests. The IBC VM fixtures call all six IBC entry points, relay application
bytes between two VMs, and explicitly dispatch self messages. They do **not**
validate IBC proofs, run light clients, or provide the Cosmos SDK transaction
cache. Native tests cover errors, malformed/oversized acknowledgements, spoofed
execute calls, tampered packet intents/endpoints, duplicate callbacks, deadlines,
late results, channel closure, and handshake restrictions.

## Run two actual local chains

The [recorded local smoke result](ibc-smoke-result.json) was verified on
2026-10-04 with native `wasmd` 0.55.1 and a Hermes binary reporting
`1.13.2+bab3b80` (from its v1.13.3 release archive). It records the exact tested
artifact SHA-256 and terminal states. All local node/relayer processes were
stopped afterward.

With a native `wasmd` v0.55.1 binary and Hermes installed:

```sh
scripts/build-wasm.sh ibc-query-example
python3 scripts/local-ibc-demo.py --wasmd /path/to/wasmd --hermes /path/to/hermes
```

The script creates fresh isolated chain homes and local test keys, starts two
single-validator chains, stores the artifact, predicts/instantiates both peer
addresses, creates IBC clients/connection/channel, and starts Hermes. It asserts
completion with 20, an overflow error acknowledgement, and a proven IBC timeout
by stopping the relayer before a third request and restarting it after its
30-second timeout. It saves `results.json`, transaction receipts, handshake and
relayer logs in the printed directory. Processes are stopped in `finally` on
both success and failure. The directory remains for inspection; it includes
**disposable local test keys**, so do not publish it wholesale. No existing
chain home or global relayer configuration is used.

Hermes polls block results because CosmWasm IBC events can omit the `message`
attribute required by its WebSocket subscription (see the [upstream Hermes
configuration](https://github.com/informalsystems/hermes/blob/v1.13.3/config.toml)).

Ports default to RPC 28657/28667, P2P 28656/28666 and gRPC 28757/28767;
`--port` changes their base. This is a local smoke harness, not a production
relayer configuration or an adversarial validator-network test. Hermes may log
that misbehaviour monitoring is unavailable when the local wasmd event omits the
client-update header; the smoke test does not establish that monitoring capability.

## Protocol and trust

- Contract API uses snake_case execute/query variants. Instantiate with
  `connection_id`, `counterparty_port` (`wasm.<peer-contract>`), and
  `timeout_seconds` (1–86400). The connection and peer port are immutable.
- Handshake requires `durable-query-1`, unordered delivery, the configured
  connection, the configured remote contract port, and the contract's own local
  port. Exactly one channel may connect. Closed channels cannot be rebound.
- Packet: `{"correlation":{"workflow_id":1,"wait_sequence":1},"value":5}`.
  Ack: `{"result":{"value":10}}` or `{"error":{"code":"overflow"}}`.
  Schemas reject unknown fields. Packets/acknowledgements are capped at 1024 bytes;
  remote error codes at 64 bytes. Invalid acknowledgements become a bounded
  `invalid_ack` workflow failure instead of leaving the IBC acknowledgement
  permanently unprocessable.
- Source acknowledgement/timeout handlers verify both endpoints and the exact
  saved data and timeout. The IBC core authenticates the original packet and
  enforces packet-sequence commitment/proof/replay rules; contract correlation
  does not replace those checks. The adapter never trusts the relayer address.
- Only IBC handlers generate an internal `deliver` execute message. Execute
  accepts this variant **only when sender equals this contract's address**.
  The runtime resolver is that same address. This lets the existing runtime
  authenticate an actual self-call without forging `MessageInfo` or exposing
  a public trusted-resolution API. There is no arbitrary-message forwarding API.
- Starting workflows is owner-only. All execute/instantiate calls reject funds.
  The example assumes the configured peer/connection are trusted; IBC verifies
  provenance, not the mathematical truth of the peer's answer. Deploy without a
  migration admin when relying on immutable behavior.

## Failure, deadlines, and rollback

Remote `error` acknowledgements become `WaitError::Remote`. The workflow's
`.await?` persists terminal failure. Both packet timeout and local deadline use
the same Unix-second timestamp. At or after the **source** deadline, either an
acknowledgement or a timeout resolves through `expire` to `WaitError::Timeout`.
If the remote chain proves timeout while the source clock has not yet reached
that deadline, the IBC commitment is consumed and the workflow stays waiting
until anyone submits `expire` at the source deadline. No arbitrary sender can
expire it early. This is an intentional consequence of retaining the runtime's
local-deadline policy without adding a privileged timeout API.

```json
{"start":{"value":5}}
{"instance":{"workflow_id":1}}
{"expire":{"workflow_id":1,"wait_sequence":1}}
```

A local expiration removes the adapter intent. Later acknowledgements/timeouts
are harmless no-ops and cannot resurrect the instance. Duplicate callbacks do
not advance the next wait. Channel closure blocks new sends; a pending result
received after closure fails with `channel_closed`, or expires if overdue.
Outstanding workflows without callbacks can always expire at their deadline.

The IBC callback's self-execute is a normal message (no caught submessage error).
On a host transaction failure, the SDK must roll back intent deletion, runtime
state and outbound messages together. A relayer can retry the acknowledgement;
a late retry follows the expiration policy. Unit/VM fixtures alone do not prove
that host rollback boundary.

**Local expiration is not remote cancellation.** A request may already have run
or may still run remotely. This demo is deliberately a pure query. Payments,
orders, refunds and other effects require a separate idempotency/compensation
protocol and application-specific acknowledgement semantics.

## Deployment outline

Use two IBC-enabled CosmWasm 2.x chains, create a connection and configure a
relayer for both chains. Store the same artifact on each chain. Predict each
contract address with `wasmd query wasm build-address` using the artifact
checksum, creator and salt, then use `wasmd tx wasm instantiate2` without
`--fix-msg`; this avoids circular peer-address configuration. Instantiate each
side with the other's predicted port and its local connection ID. Establish an
unordered channel on those two ports with version `durable-query-1`, then relay
packets and acknowledgements after executing `start` on the source. Query its
instance until `Completed`; decode the output Binary to the JSON number 20.

Contract API reference is pinned to the workspace lockfile's
[cosmwasm-std 2.3.5 IBC types](https://docs.rs/cosmwasm-std/2.3.5/src/cosmwasm_std/ibc.rs.html).
The VM compatibility gate uses cosmwasm-vm 2.2.2. This is IBC classic's six-entrypoint
contract protocol, not ICS-20 transfers, callbacks middleware, or IBC v2.
