#!/usr/bin/env bash
# Shared wasmvm 2.2.4 policy for Rust 1.99 output; source from artifact builders.
ensure_wasmd_binaryen() {
  if [[ -z "${WASM_OPT:-}" ]]; then
    [[ "$(uname -s)/$(uname -m)" == "Darwin/arm64" ]] || {
      echo 'Set WASM_OPT to Binaryen version 123 on this platform.' >&2; return 1;
    }
    local tool_dir
    tool_dir="$(mktemp -d)"
    curl -fsSL 'https://github.com/WebAssembly/binaryen/releases/download/version_123/binaryen-version_123-arm64-macos.tar.gz' -o "$tool_dir/binaryen.tar.gz"
    echo "74428be348c1a09863e7b642a1fa948cabf8ec9561052233d8288e941951725b  $tool_dir/binaryen.tar.gz" | shasum -a 256 -c -
    tar -xzf "$tool_dir/binaryen.tar.gz" -C "$tool_dir"
    WASM_OPT="$tool_dir/binaryen-version_123/bin/wasm-opt"
  fi
  [[ "$("$WASM_OPT" --version)" == 'wasm-opt version 123 (version_123)' ]] || {
    echo 'Binaryen 123 required' >&2; return 1;
  }
}

normalize_wasmd() {
  "$WASM_OPT" "$1" --enable-sign-ext --enable-bulk-memory --enable-nontrapping-float-to-int \
    --signext-lowering --llvm-memory-copy-fill-lowering --llvm-nontrapping-fptoint-lowering \
    --disable-sign-ext --disable-bulk-memory --disable-nontrapping-float-to-int -o "$2"
}
