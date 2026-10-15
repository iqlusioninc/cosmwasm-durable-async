#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
scenario="${1:-all}"
[[ $# -le 1 ]] || { echo 'Usage: bash scripts/native-dca-demo.sh happy|funding|liquidity|cancel|restart|all' >&2; exit 2; }
case "$scenario" in
  happy|funding|liquidity|cancel|restart|all) ;;
  *) echo 'Usage: bash scripts/native-dca-demo.sh happy|funding|liquidity|cancel|restart|all' >&2; exit 2 ;;
esac
export GOTOOLCHAIN=go1.23.6
export NATIVE_DCA_DEMO=1
export NATIVE_DCA_SCENARIO="$scenario"
if [[ -z "${NATIVE_DCA_OUTPUT:-}" ]]; then
  NATIVE_DCA_OUTPUT="$(mktemp -d "${TMPDIR:-/tmp}/native-dca-demo.XXXXXX")"
fi
[[ -d "$NATIVE_DCA_OUTPUT" ]] || { echo 'NATIVE_DCA_OUTPUT must name an existing directory' >&2; exit 2; }
NATIVE_DCA_OUTPUT="$(cd "$NATIVE_DCA_OUTPUT" && pwd -P)"
export NATIVE_DCA_OUTPUT
printf 'Local test-token scenarios: %s\nReceipts and logs: %s\n' "$scenario" "$NATIVE_DCA_OUTPUT"
bash contracts/native-dca/build-wasmd.sh
if [[ -n "${GOWORK:-}" && "$GOWORK" != off ]]; then
  [[ -f "$GOWORK" ]] || { echo 'GOWORK must name the retained patched workspace' >&2; exit 2; }
  cd integration/wasmd
  go test -tags upstream -count=1 -v -timeout 8m -run '^TestNativeDCADemo$' .
else
  # A cold run builds the checksum-pinned patched host/VM and cleans its own
  # large temporary directories unless NATIVE_ASYNC_KEEP=1 was selected.
  NATIVE_ASYNC_TEST_RUN='^TestNativeDCADemo$' bash integration/wasmd/test.sh
fi
