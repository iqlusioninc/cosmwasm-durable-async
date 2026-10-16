#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
demo_root="$PWD"
usage='Usage: bash scripts/native-auction-demo.sh happy|funding|restart|expiry|all'
[[ $# -le 1 ]] || { echo "$usage" >&2; exit 2; }
scenario="${1:-all}"
case "$scenario" in
  happy|funding|restart|expiry|all) ;;
  *) echo "$usage" >&2; exit 2 ;;
esac
export GOTOOLCHAIN=go1.23.6
export NATIVE_AUCTION_DEMO=1
export NATIVE_AUCTION_SCENARIO="$scenario"
if [[ -z "${NATIVE_AUCTION_OUTPUT:-}" ]]; then
  NATIVE_AUCTION_OUTPUT="$(mktemp -d "${TMPDIR:-/tmp}/native-auction-demo.XXXXXX")"
fi
[[ -d "$NATIVE_AUCTION_OUTPUT" ]] || { echo 'NATIVE_AUCTION_OUTPUT must name an existing directory' >&2; exit 2; }
NATIVE_AUCTION_OUTPUT="$(cd "$NATIVE_AUCTION_OUTPUT" && pwd -P)"
export NATIVE_AUCTION_OUTPUT
if [[ -n "${GOWORK:-}" && "$GOWORK" != off ]]; then
  [[ -f "$GOWORK" ]] || { echo 'GOWORK must name the retained patched workspace' >&2; exit 2; }
  GOWORK="$(cd "$(dirname "$GOWORK")" && pwd -P)/$(basename "$GOWORK")"
  export GOWORK
fi
NATIVE_AUCTION_RUN="$(mktemp -d "$NATIVE_AUCTION_OUTPUT/run.XXXXXX")"
export NATIVE_AUCTION_RUN
printf 'Local test-token auction scenarios: %s\nReceipts and logs: %s\n' "$scenario" "$NATIVE_AUCTION_OUTPUT"
bash contracts/native-batch-auction/build-wasmd.sh
if [[ -n "${GOWORK:-}" && "$GOWORK" != off ]]; then
  cd integration/wasmd
  go test -tags upstream -count=1 -v -timeout 8m -run '^TestNativeAuctionDemo$' .
else
  # The cold path builds the pinned patched host/VM. It removes its own large
  # temporary build directories unless NATIVE_ASYNC_KEEP=1 was selected.
  NATIVE_ASYNC_TEST_RUN='^TestNativeAuctionDemo$' bash integration/wasmd/test.sh
fi
cd "$demo_root"
node scripts/verify-auction-demo.mjs "$NATIVE_AUCTION_RUN" "$scenario" artifacts/native_batch_auction.wasm
