#!/usr/bin/env bash
# wasmvm 2.2.4 normalization for Rust 1.99 output. Binaryen 123 is pinned.
set -euo pipefail
cd "$(dirname "$0")/../.."
source scripts/wasmd-normalize.sh
ensure_wasmd_binaryen
artifact_target="${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/release"
cargo build --locked -p native-fulfill-example --release --target wasm32-unknown-unknown
cargo build --locked -p native-fulfill-example --example native_service --release --target wasm32-unknown-unknown
cargo build --locked -p native-fulfill-example --example native_fault --release --target wasm32-unknown-unknown
mkdir -p artifacts
normalize_wasmd "$artifact_target/native_fulfill_example.wasm" artifacts/native_fulfill_example.wasm
normalize_wasmd "$artifact_target/examples/native_service.wasm" artifacts/native_service.wasm
normalize_wasmd "$artifact_target/examples/native_fault.wasm" artifacts/native_fault.wasm
shasum -a 256 artifacts/native_fulfill_example.wasm artifacts/native_service.wasm artifacts/native_fault.wasm
