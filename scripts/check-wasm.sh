#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
scripts/build-wasm.sh fulfill-example ibc-query-example
export IBC_WASM="$root/target/artifacts/ibc_query_example.wasm"
export DURABLE_WASM="$root/target/artifacts/fulfill_example.wasm"
cargo +1.85.1 test --manifest-path tools/vm-tests/Cargo.toml --locked -- --nocapture
