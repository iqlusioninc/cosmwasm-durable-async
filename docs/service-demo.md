# Delayed service demo

This example performs **no payments or shipments**. An owner-operated test service stores requests from one bound workflow contract. A separate process later delivers illustrative results. Fulfillment authorizes callbacks from the service contract address, not the driver account. One service instance handles both operation types; real integrations can use separate adapters/services.

## Quickstart

Run native `wasmd` v0.55.1 (CosmWasm VM 2.2.4) and Python 3.10+ on macOS or Linux. Use Wasm artifacts normalized for CosmWasm VM 2.x; raw artifacts from recent Rust compilers may use unsupported Wasm instructions. Build both contracts using the VM validation lane's `scripts/build-wasm.sh` when available, or:

```sh
rustup toolchain install 1.85.1 --target wasm32-unknown-unknown
RUSTFLAGS='-C target-feature=-reference-types,-multivalue,-bulk-memory' \
  cargo +1.85.1 build --locked --release --target wasm32-unknown-unknown \
  --target-dir target/wasm-compatible -p fulfill-example -p demo-service
mkdir -p target/demo-artifacts
# wasm-opt is Binaryen version 123. Lower and normalize both artifacts.
for contract in fulfill_example demo_service; do
  wasm-opt "target/wasm-compatible/wasm32-unknown-unknown/release/$contract.wasm" \
    -Oz --enable-bulk-memory --enable-reference-types --llvm-memory-copy-fill-lowering \
    -o "target/demo-artifacts/$contract.wasm"
done
python3 scripts/local-service-demo.py --wasmd /path/to/wasmd \
  --artifacts target/demo-artifacts
```

The runner creates a fresh chain home, a disposable local test-keyring account and validator, and uses loopback RPC/P2P ports 27657/27656. Override `--port` if occupied. It never reuses a chain directory and stops the node when it exits. `--home /tmp/my-new-demo` chooses a nonexisting directory. Genesis funds are local test tokens.

The script stores and instantiates both Wasm contracts, binds the service once, and asserts:

* Payment and shipment callbacks complete a workflow in separate transactions.
* A remote application error produces terminal `Failed` despite transaction success.
* A low-gas delivery transaction fails with code 11 and leaves the queue and checkpoint unchanged.
* Malformed callback bytes fail the transaction; the service queue and workflow checkpoint survive. A later valid delivery completes the same workflow.
* Height expiration produces terminal failure, followed by verified stale-request cleanup.
* Every driver invocation is a fresh process: recovery uses chain queries and a transaction journal.

The final `PASS` prints the retained chain home. Inspect `results.json`, `addresses.json`, `node.log`, and the chain database there. No background chain is left running. The script does not measure performance or prove distributed exactly-once execution.

## Driver and wire messages

For an already running local chain, repeatedly run:

```sh
python3 scripts/service-driver.py --wasmd /path/to/wasmd --home /tmp/chain \
  --node tcp://127.0.0.1:27657 --workflow wasm1... --service wasm1... \
  --journal /tmp/chain/driver.json
```

Each invocation reconciles a prior transaction, queries the oldest pending request and current workflow state, then submits at most one transaction. Wait for confirmation before invoking again. `--mode remote-error` delivers a synthetic application failure; `--mode invalid` deliberately sends malformed callback bytes. After a confirmed failed transaction, rerun with `--mode success` to retry. Expired requests trigger `Expire`; terminal or superseded requests trigger `Prune`. Application failure is not retried.

Wire enums retain the existing example's capitalized JSON variants: `{"Pending":{"start_after":null,"limit":30}}`, `{"Bind":{"workflow":"wasm1..."}}`, and `{"Deliver":{"id":1,"outcome":{"Error":{"code":"demo","message":"declined"}}}}`. Pending pagination returns ascending service-local IDs with exclusive `start_after`; limits clamp to 1–100. `Bind` is owner-only and permanent. `Begin` is workflow-only, rejects reused correlations and bounds request bytes. `Deliver` is owner-only. `Prune` is permissionless but queries the bound workflow to prove the request is no longer live. All entry points reject funds.

The driver locks its journal, flushes intent before broadcasting, records the transaction hash, and refuses to retry if a crash or transport failure leaves an ambiguous broadcast. An unindexed hash remains unresolved. **Do not delete such a journal to force retry.** Inspect account sequence, chain transactions and workflow/service state first; record the confirmed transaction hash into the journal only after establishing which transaction was submitted. For this disposable demo, rebuilding a fresh chain is also possible. Journal scope is tied to the chain endpoint, account home/key and contract addresses. Multiple journals/account users are not coordinated.

The service removes pending work before emitting the callback; an ordinary callback execution failure rolls that removal back atomically. It never catches callback errors with `reply_on_error`. A transaction success can still mean the workflow stored a terminal application error. Terminal workflow results are stored JSON; they do not automatically dispatch application messages such as fund transfers.

## Boundaries

The driver is a local test tool using a test keyring, fixed gas/fees and synthetic service results. It is not a production payment processor, scheduler, or external-effect deduplication system. Queue ordering can block behind a malformed oldest request. Seen-correlation tombstones and workflow terminal records are retained indefinitely. Production systems need retention, monitoring, per-operation result validation, real service authentication and external idempotency policies.

Run fast checks with:

```sh
cargo test -p demo-service
python3 -m unittest discover -s scripts -p test_service_driver.py
```
