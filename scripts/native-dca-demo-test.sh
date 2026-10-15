#!/usr/bin/env bash
# Exercise the runner's output path across its working-directory change without
# rebuilding Wasm or starting a validator.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/native-dca-script-test.XXXXXX")"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/scripts" "$fixture/integration/wasmd" "$fixture/bin" "$fixture/output with spaces"
cp "$root/scripts/native-dca-demo.sh" "$fixture/scripts/"
touch "$fixture/go.work"
cat > "$fixture/bin/bash" <<'STUB'
#!/bin/bash
set -euo pipefail
[[ "$*" == contracts/native-dca/build-wasmd.sh ]]
STUB
cat > "$fixture/bin/go" <<'STUB'
#!/bin/bash
set -euo pipefail
[[ "$PWD" == "$DEMO_TEST_ROOT/integration/wasmd" ]]
[[ "$NATIVE_DCA_OUTPUT" == "$DEMO_TEST_ROOT/output with spaces" ]] || {
  printf 'wrong output directory after cd: %s\n' "$NATIVE_DCA_OUTPUT" >&2
  exit 1
}
printf 'receipt written after cd\n' > "$NATIVE_DCA_OUTPUT/receipt.txt"
STUB
chmod +x "$fixture/bin/bash" "$fixture/bin/go"
fixture="$(cd "$fixture" && pwd -P)"
PATH="$fixture/bin:$PATH" DEMO_TEST_ROOT="$fixture" GOWORK="$fixture/go.work" \
  NATIVE_DCA_OUTPUT='output with spaces' /bin/bash "$fixture/scripts/native-dca-demo.sh" happy
[[ "$(< "$fixture/output with spaces/receipt.txt")" == 'receipt written after cd' ]]
printf 'PASS: existing relative output directory with spaces survives runner cd\n'
