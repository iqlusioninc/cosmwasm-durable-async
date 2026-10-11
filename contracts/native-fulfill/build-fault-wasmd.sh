#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${WASM_OPT:?Set WASM_OPT to Binaryen 123 wasm-opt}"
[[ "$("$WASM_OPT" --version)" == 'wasm-opt version 123 (version_123)' ]]
cargo build --locked -p native-fulfill-example --example native_fault --release --target wasm32-unknown-unknown
mkdir -p artifacts
"$WASM_OPT" target/wasm32-unknown-unknown/release/examples/native_fault.wasm \
  --enable-sign-ext --enable-bulk-memory --enable-nontrapping-float-to-int \
  --signext-lowering --llvm-memory-copy-fill-lowering --llvm-nontrapping-fptoint-lowering \
  --disable-sign-ext --disable-bulk-memory --disable-nontrapping-float-to-int -o artifacts/native_fault.wasm
shasum -a 256 artifacts/native_fault.wasm
