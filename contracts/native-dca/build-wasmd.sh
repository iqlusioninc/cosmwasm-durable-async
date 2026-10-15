#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
source scripts/wasmd-normalize.sh
ensure_wasmd_binaryen
artifact_target="${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/release"
cargo build --locked -p native-dca -p mock-exchange --release --target wasm32-unknown-unknown
cargo build --locked -p native-dca --example dca_fault --release --target wasm32-unknown-unknown
mkdir -p artifacts
normalize_wasmd "$artifact_target/native_dca.wasm" artifacts/native_dca.wasm
normalize_wasmd "$artifact_target/mock_exchange.wasm" artifacts/mock_exchange.wasm
normalize_wasmd "$artifact_target/examples/dca_fault.wasm" artifacts/dca_fault.wasm
shasum -a 256 artifacts/native_dca.wasm artifacts/mock_exchange.wasm artifacts/dca_fault.wasm
