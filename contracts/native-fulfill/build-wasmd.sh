#!/usr/bin/env bash
# wasmvm 2.2.4 normalization for Rust 1.99 output. Binaryen 123 is pinned.
set -euo pipefail
cd "$(dirname "$0")/../.."
if [[ -z "${WASM_OPT:-}" ]]; then
  [[ "$(uname -s)/$(uname -m)" == "Darwin/arm64" ]] || {
    echo 'Set WASM_OPT to Binaryen version 123 on this platform.' >&2; exit 1;
  }
  tool_dir="$(mktemp -d)"
  curl -fsSL 'https://github.com/WebAssembly/binaryen/releases/download/version_123/binaryen-version_123-arm64-macos.tar.gz' -o "$tool_dir/binaryen.tar.gz"
  echo "74428be348c1a09863e7b642a1fa948cabf8ec9561052233d8288e941951725b  $tool_dir/binaryen.tar.gz" | shasum -a 256 -c -
  tar -xzf "$tool_dir/binaryen.tar.gz" -C "$tool_dir"
  WASM_OPT="$tool_dir/binaryen-version_123/bin/wasm-opt"
fi
[[ "$("$WASM_OPT" --version)" == 'wasm-opt version 123 (version_123)' ]] || { echo 'Binaryen 123 required' >&2; exit 1; }
cargo build --locked -p native-fulfill-example --release --target wasm32-unknown-unknown
cargo build --locked -p native-fulfill-example --example native_service --release --target wasm32-unknown-unknown
mkdir -p artifacts
normalize() {
  "$WASM_OPT" "$1" --enable-sign-ext --enable-bulk-memory --enable-nontrapping-float-to-int \
    --signext-lowering --llvm-memory-copy-fill-lowering --llvm-nontrapping-fptoint-lowering \
    --disable-sign-ext --disable-bulk-memory --disable-nontrapping-float-to-int -o "$2"
}
normalize target/wasm32-unknown-unknown/release/native_fulfill_example.wasm artifacts/native_fulfill_example.wasm
normalize target/wasm32-unknown-unknown/release/examples/native_service.wasm artifacts/native_service.wasm
shasum -a 256 artifacts/native_fulfill_example.wasm artifacts/native_service.wasm
