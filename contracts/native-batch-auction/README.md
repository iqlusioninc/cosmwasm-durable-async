# Native batch auction local tutorial

This Stage A example sells a fixed inventory in one sealed-bid, uniform-price
auction. It is capped at 256 bidder addresses, 256 price ticks and 32 distinct
bid/tick records per poll. One address may commit once; that cap provides no
Sybil resistance. There is no non-reveal penalty and no guaranteed service or
fairness. The seller may decline to fund or resume settlement. Fixed-expiry
principal refunds are the fallback.

## Run locally

From the repository root:

```sh
bash scripts/native-auction-demo.sh happy
bash scripts/native-auction-demo.sh funding
bash scripts/native-auction-demo.sh restart
bash scripts/native-auction-demo.sh expiry
bash scripts/native-auction-demo.sh all
```

The runner builds the real normalized auction artifacts and selects
`TestNativeAuctionDemo` with `NATIVE_AUCTION_DEMO=1` and
`NATIVE_AUCTION_SCENARIO`. Only `native_batch_auction.wasm` is used by the
tutorial; `auction_fault.wasm` is a separate acceptance-test artifact. The node
must be a loopback-only child of the test executable linked to the patched
wasmd/VM. The scenario tests use deterministic block permits to expose exact
committed restart windows, not to promise scheduler liveness.

The [adapter guide](../../integration/wasmd/README.md) lists prerequisites,
pinned dependencies and cold bootstrap downloads. The runner always rebuilds
the auction artifacts. A retained, valid absolute `GOWORK` reuses the patched
host/VM with the selected `GOPATH` and `GOCACHE`; otherwise it uses the cold
bootstrap. The builder respects `CARGO_TARGET_DIR` and `WASM_OPT`. Go is pinned
to 1.23.6. No public network, production funds or deployment are involved.

Builders write the checkout's shared `artifacts/` files in place. Serialize
builders/demo scripts with other host/network tests or demos using that checkout,
or use separate clones with separate artifact directories. `CARGO_TARGET_DIR`
isolates Rust intermediates but does not isolate the published `artifacts/`.

`NATIVE_AUCTION_OUTPUT` may name an existing directory; the runner resolves it
before changing into `integration/wasmd`, including paths containing spaces.
Relative output paths are relative to the repository root. If unset, the runner
creates and prints a temporary output directory. Each invocation creates a fresh
`run.*` directory, whose `suite-result.json` identifies the exact selected
scenarios. The runner verifies original signed journals, salt snapshots,
source/artifact/config, final claims and owned-child exits; a missing/skipped
test or stale success from an earlier run fails. Scenarios retain small
query/transaction receipts and node logs there, while removing
owned chain homes and their test validator keys. Non-test signing keys must
never be persisted.

## Fixture and expected result

The shared acceptance book has supply 5 lots, 10 `usale` per lot, reserve price
2 `uquote` per lot, tick size 1, three ticks and batch size 2. Five bids use:

| Commit index | Lots | Tick | Collateral `uquote` | Revealed |
| --- | ---: | ---: | ---: | --- |
| 0 | 2 | 2 | 8 | yes |
| 1 | 2 | 1 | 6 | yes |
| 2 | 2 | 1 | 6 | yes |
| 3 | 1 | 0 | 2 | yes |
| 4 | 1 | 2 | 7 | no |

The hand-calculated result is clearing price 3, allocations `[2,2,1,0,0]`, sale
payouts `[20,20,10,0,0]`, and quote refunds `[2,0,3,2,7]`. Seller proceeds are
15 quote units: collateral 29 = refunds 14 + seller 15; sale inventory 50 is
fully allocated. Commit-order dust gives the first marginal bidder the extra
lot. This deterministic rule is sensitive to ordering.

The demo calls `scripts/auction-commitment.mjs` with exact chain, contract,
bidder, beneficiary and integer terms. The helper uses the OS cryptographic RNG
for missing 32-byte salts and emits digest/salt as hex and CosmWasm base64.
Per-run generated inputs and outputs are saved once in a fsynced, exclusive-create
0600 `commitments.json` and reused exactly for reveals. Do not regenerate a salt
after committing. Commitment encoding is
versioned binary SHA-256, not a hash of JSON. Reproduce the client vectors with:

```sh
node --test scripts/auction-commitment.test.mjs
```

## Funding, recovery and claims

The seller instantiates with no chain-level admin and creates exactly one
auction, attaching exact sale inventory and separate gas funds. The host task
ID comes from registration; no guessed ID controls the auction. Collateral
enters the contract, while gas enters host escrow. The contract's raw bank
balance must not be used to infer principal or gas entitlements.

The local Task 6 fixture uses a 100,000,000 SDK gas attempt cap, including a
70,000,000 host reserve, and a 17,920-byte continuation allocation. Its initial
price is .001 `ugas` per SDK gas. These are measured local fixture settings,
not universal production prices or maximum-cost claims for every allowed book.
For registration H and expiry E, quote exact funding from the live config:

```text
lease = allocated_bytes * (E-H) * storage_price + cleanup_price
maximum_attempt = ceil(current_gas_price * gas_per_poll)
deposit = lease + maximum_attempt * desired_attempts
```

The happy scenario checks three nonempty allocation batches, three claim-building
batches and the oracle's payouts. Funding pauses with `insufficient_funds` after
partial allocation; third-party top-up and seller ceiling increase each leave it
paused until explicit seller resume. A calibrated local 500-ugas margin lets
the initial timer and price scan finish before this pause; it is not a worst-case
gas bound or an organic price-limit experiment.

Restart compares exact committed queries in fresh PIDs before handshake/replay
at both H23 (nonempty partial allocation) and H29 (finalized before claims),
before releasing each next block. Expiry rejects provisional claims at E-1,
then returns every bidder's full collateral and the seller's full inventory
after host pruning. All scenarios verify actual bank payouts and repeated-claim
rejection. Independent auction state-sync tests are a separate network layer,
recorded in the verification receipt.

Application state `finalized` unlocks immutable bidder/seller claims; host task
status is separate. Before E, provisional results cannot be claimed. At or after
E, an unfinished auction becomes `aborted`, including a lazy transition during
a claim, with full principal refunds. A finalized auction stays finalized after
E. Claim rights survive host-task pruning and have no expiry or administrative
sweep. Permissionless callers can trigger claims but cannot change recipients;
duplicate claims fail. Gas refunds remain a separate host operation.

Signed transactions reuse the generic tutorial journal: signed bytes and hash
are durable before submission; attempted state is durable before RPC. A lost
response is resolved solely by looking up that original hash. An unresolved
outcome halts the scenario. Never re-sign or rebroadcast an uncertain intent.

Final source/artifact/config identity, command results and explicit evidence
gaps belong in the [verification receipt](../../docs/reviews/2026-10-15-native-batch-auction-verification.md).
