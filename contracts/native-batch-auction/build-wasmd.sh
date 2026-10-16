#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
source scripts/wasmd-normalize.sh
ensure_wasmd_binaryen
artifact_target="${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/release"
cargo build --locked -p native-batch-auction --release --target wasm32-unknown-unknown
mkdir -p artifacts
normalize_wasmd "$artifact_target/native_batch_auction.wasm" artifacts/native_batch_auction.wasm
cargo build --locked -p native-batch-auction --example auction_fault --features library --release --target wasm32-unknown-unknown
normalize_wasmd "$artifact_target/examples/auction_fault.wasm" artifacts/auction_fault.wasm
shasum -a 256 artifacts/native_batch_auction.wasm artifacts/auction_fault.wasm
