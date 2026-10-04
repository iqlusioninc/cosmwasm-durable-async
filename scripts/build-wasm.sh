#!/usr/bin/env bash
# Produces the artifacts validated for the CosmWasm 2.2 VM feature set.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
wasm_opt=${WASM_OPT:-"$root/target/tools/binaryen-version_123/bin/wasm-opt"}
if [[ ! -x "$wasm_opt" ]]; then
  echo 'Run scripts/install-binaryen.sh first (or set WASM_OPT to Binaryen 123).' >&2
  exit 1
fi
[[ "$("$wasm_opt" --version)" == 'wasm-opt version 123 (version_123)' ]] || { echo 'Binaryen 123 required' >&2; exit 1; }
if [[ $# == 0 ]]; then set -- fulfill-example; fi
mkdir -p target/artifacts
for package in "$@"; do
  # This repository names contract libraries after their package (hyphen -> underscore).
  [[ "$package" =~ ^[a-z0-9-]+$ ]] || { echo 'Invalid package name' >&2; exit 1; }
  cargo +1.99.0 build -p "$package" --release --target wasm32-unknown-unknown --target-dir "$root/target" --locked
  name=${package//-/_}
  "$wasm_opt" "target/wasm32-unknown-unknown/release/$name.wasm" -Oz \
    --enable-bulk-memory --enable-reference-types --llvm-memory-copy-fill-lowering \
    -o "target/artifacts/$name.wasm"
  python3 - "target/artifacts/$name.wasm" <<'PY'
import hashlib, pathlib, sys
path = pathlib.Path(sys.argv[1])
line = f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n"
path.with_suffix('.wasm.sha256').write_text(line)
print(line, end='')
PY
done
