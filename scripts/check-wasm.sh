#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
scripts/build-wasm.sh fulfill-example
export DURABLE_WASM="$root/target/artifacts/fulfill_example.wasm"
cargo +1.99.0 test --manifest-path tools/vm-tests/Cargo.toml --locked -- --nocapture
