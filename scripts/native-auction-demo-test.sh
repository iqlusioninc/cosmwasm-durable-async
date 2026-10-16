#!/usr/bin/env bash
# Test the runner's path and opt-in contract without compiling Wasm or a node.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
[[ -f "$root/scripts/native-auction-demo.sh" ]] || {
  echo 'FAIL: native auction runner is missing' >&2
  exit 1
}
fixture="$(mktemp -d "${TMPDIR:-/tmp}/native-auction-script-test.XXXXXX")"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/scripts" "$fixture/integration/wasmd" "$fixture/bin" "$fixture/output with spaces"
cp "$root/scripts/native-auction-demo.sh" "$fixture/scripts/"
cp "$root/scripts/verify-auction-demo.mjs" "$root/scripts/auction-commitment.mjs" "$fixture/scripts/"
touch "$fixture/go.work"
cat > "$fixture/scripts/write-fixture.mjs" <<'STUB'
import { createHash } from 'node:crypto';
import { writeFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { commitmentDigest } from './auction-commitment.mjs';
const hash = b => createHash('sha256').update(b).digest('hex');
const run = process.env.NATIVE_AUCTION_RUN;
const names = process.env.NATIVE_AUCTION_SCENARIO === 'all' ? ['happy','funding','restart','expiry'] : [process.env.NATIVE_AUCTION_SCENARIO];
const index = [];
for (const name of names) {
  const folder = join(run,name); mkdirSync(folder);
  const tx = {hash:hash('signed fixture').toUpperCase(),outcome:'committed',check_tx_code:0,deliver_tx_code:0,height:9};
  const final = {View:{auction:{state:name==='expiry'?'aborted':'finalized',seller_claimed:true},host:{unavailable:{error:'pruned'}}},Bids:[{claimed:true}],Task:null};
  const receipt = {scenario:name,passed:true,owned_children_exited:true,source_revision:'a'.repeat(40),source_dirty:false,artifact_sha256:hash('fixture wasm'),host_config:{},funding_config:{denom:'ugas'},node_config:{host_reserve:1},children:[{pid:1,exited:true},{pid:2,exited:true}],transactions:[tx],snapshots:[{capture:{State:final}}]};
  writeFileSync(join(folder,'receipt.json'),JSON.stringify(receipt));
  writeFileSync(join(folder,'tx-001.json'),JSON.stringify({stage:'resolved',hash:tx.hash,tx_hex:Buffer.from('signed fixture').toString('hex'),receipt:tx}));
  const salts = Array.from({length:5},(_,i)=>{const input={chain_id:'local',contract:'contract',bidder:'bidder',beneficiary:'beneficiary',auction_id:'1',collateral:'8',quantity_lots:'2',price_tick:2};const salt_hex=String(i+1).padStart(64,'0');return {input,output:{salt_hex,digest_hex:commitmentDigest({...input,salt_hex})}};});
  writeFileSync(join(folder,'commitments.json'),JSON.stringify(salts),{mode:0o600});
  index.push({scenario:name,receipt:`${name}/receipt.json`});
}
writeFileSync(join(run,'suite-result.json'),JSON.stringify(index));
STUB
cat > "$fixture/bin/bash" <<'STUB'
#!/bin/bash
set -euo pipefail
[[ "$GOTOOLCHAIN" == go1.23.6 ]]
[[ "$NATIVE_AUCTION_DEMO" == 1 ]]
[[ "$NATIVE_AUCTION_SCENARIO" == "$DEMO_TEST_SCENARIO" ]]
[[ "$NATIVE_AUCTION_OUTPUT" == "$DEMO_TEST_ROOT/output with spaces" ]]
case "$*" in
  contracts/native-batch-auction/build-wasmd.sh)
    mkdir -p "$DEMO_TEST_ROOT/artifacts"
    printf 'fixture wasm' > "$DEMO_TEST_ROOT/artifacts/native_batch_auction.wasm"
    printf 'build\n' >> "$DEMO_TEST_LOG"
    ;;
  integration/wasmd/test.sh)
    [[ "$PWD" == "$DEMO_TEST_ROOT" ]]
    [[ "$NATIVE_ASYNC_TEST_RUN" == '^TestNativeAuctionDemo$' ]]
    printf 'cold:%s\n' "$NATIVE_AUCTION_SCENARIO" >> "$DEMO_TEST_LOG"
    node "$DEMO_TEST_ROOT/scripts/write-fixture.mjs"
    ;;
  *) printf 'unexpected bash command: %s\n' "$*" >&2; exit 1 ;;
esac
STUB
cat > "$fixture/bin/go" <<'STUB'
#!/bin/bash
set -euo pipefail
[[ "$PWD" == "$DEMO_TEST_ROOT/integration/wasmd" ]]
[[ "$GOTOOLCHAIN" == go1.23.6 && "$NATIVE_AUCTION_DEMO" == 1 ]]
[[ "$NATIVE_AUCTION_SCENARIO" == "$DEMO_TEST_SCENARIO" ]]
[[ "$NATIVE_AUCTION_OUTPUT" == "$DEMO_TEST_ROOT/output with spaces" ]]
[[ "$*" == 'test -tags upstream -count=1 -v -timeout 8m -run ^TestNativeAuctionDemo$ .' ]]
if [[ "${DEMO_TEST_OUTCOME:-}" == missing || "${DEMO_TEST_OUTCOME:-}" == skipped ]]; then
  printf 'PASS: no auction scenario executed (%s)\n' "$DEMO_TEST_OUTCOME"
  exit 0
fi
printf 'retained:%s\n' "$NATIVE_AUCTION_SCENARIO" >> "$DEMO_TEST_LOG"
printf 'receipt written after cd\n' > "$NATIVE_AUCTION_OUTPUT/receipt.txt"
node "$DEMO_TEST_ROOT/scripts/write-fixture.mjs"
STUB
chmod +x "$fixture/bin/bash" "$fixture/bin/go"
fixture="$(cd "$fixture" && pwd -P)"
export PATH="$fixture/bin:$PATH" DEMO_TEST_ROOT="$fixture"
export NATIVE_AUCTION_OUTPUT='output with spaces'
export DEMO_TEST_LOG="$fixture/calls.txt"
for scenario in happy funding restart expiry all; do
  : > "$DEMO_TEST_LOG"
  DEMO_TEST_SCENARIO="$scenario" GOWORK="$fixture/go.work" \
    /bin/bash "$fixture/scripts/native-auction-demo.sh" "$scenario"
  [[ "$(< "$DEMO_TEST_LOG")" == $'build\nretained:'"$scenario" ]]
  [[ "$(< "$fixture/output with spaces/receipt.txt")" == 'receipt written after cd' ]]
done
: > "$DEMO_TEST_LOG"
DEMO_TEST_SCENARIO=all GOWORK="$fixture/go.work" \
  /bin/bash "$fixture/scripts/native-auction-demo.sh"
[[ "$(< "$DEMO_TEST_LOG")" == $'build\nretained:all' ]]
for workspace in off ''; do
  : > "$DEMO_TEST_LOG"
  DEMO_TEST_SCENARIO=expiry GOWORK="$workspace" \
    /bin/bash "$fixture/scripts/native-auction-demo.sh" expiry
  [[ "$(< "$DEMO_TEST_LOG")" == $'build\ncold:expiry' ]]
done
expect_rejection() {
  : > "$DEMO_TEST_LOG"
  local status=0
  /bin/bash "$fixture/scripts/native-auction-demo.sh" "$@" > "$fixture/error.txt" 2>&1 || status=$?
  [[ "$status" == 2 ]] || { printf 'expected exit 2, got %s\n' "$status" >&2; exit 1; }
  [[ ! -s "$DEMO_TEST_LOG" ]] || { echo 'invalid invocation started build/test' >&2; exit 1; }
}
expect_rejection invalid
expect_rejection happy extra
NATIVE_AUCTION_OUTPUT=missing expect_rejection happy
GOWORK="$fixture/missing.work" expect_rejection happy
# Plant a valid prior success directly in the reused output directory as well
# as the earlier run.* successes. Neither may satisfy a fresh invocation.
NATIVE_AUCTION_RUN="$fixture/output with spaces" NATIVE_AUCTION_SCENARIO=happy \
  node "$fixture/scripts/write-fixture.mjs"
for outcome in missing skipped; do
  status=0
  DEMO_TEST_OUTCOME="$outcome" DEMO_TEST_SCENARIO=happy GOWORK="$fixture/go.work" \
    /bin/bash "$fixture/scripts/native-auction-demo.sh" happy > "$fixture/error.txt" 2>&1 || status=$?
  [[ "$status" == 1 ]] || { printf 'FAIL: %s auction test was accepted as a demo pass\n' "$outcome" >&2; exit 1; }
done
printf 'PASS: auction scenarios, opt-in gates, cold/retained paths, relative output with spaces, rejected inputs, and stale/missing/skipped evidence rejection\n'
