#!/usr/bin/env bash
# Source from test scripts to export the isolated, patched VM module path.
set -euo pipefail
[[ "$(rustc --version)" == 'rustc 1.99.0 (b940084d7 2026-09-28)' ]] || { echo 'Requires tested rustc 1.99.0 (b940084d7)' >&2; exit 1; }
vm_build_dir="$(mktemp -d /private/tmp/native-async-vm.XXXXXX)"
vm_source_dir="$(GOWORK=off go list -m -f '{{.Dir}}' github.com/CosmWasm/wasmvm/v2)"
[[ "$(GOWORK=off go list -m -f '{{.Version}}' github.com/CosmWasm/wasmvm/v2)" == v2.2.4 ]]
cp -R "$vm_source_dir" "$vm_build_dir/wasmvm"
chmod -R u+w "$vm_build_dir/wasmvm"
curl --fail -L https://codeload.github.com/CosmWasm/cosmwasm/tar.gz/refs/tags/v2.2.2 -o "$vm_build_dir/core.tar.gz"
[[ "$(shasum -a 256 "$vm_build_dir/core.tar.gz" | cut -d ' ' -f 1)" == 54a75734ef751f76381f08fd595600f4321e6a480b69bf3ceb7debb76dc9f9e9 ]]
tar xzf "$vm_build_dir/core.tar.gz" -C "$vm_build_dir"
GOWORK=off go run ./cmd/patch-vm-boundary "$vm_build_dir/wasmvm" "$vm_build_dir/cosmwasm-2.2.2"
export CARGO_HOME="${NATIVE_ASYNC_CARGO_HOME:-$vm_build_dir/cargo}"
export CARGO_TARGET_DIR="$vm_build_dir/target"
# Patcher changes only the six exact v2.2.2 git entries to local paths. All
# transitive versions/checksums remain unchanged; no unlocked build is allowed.
cargo build --release --locked --manifest-path "$vm_build_dir/wasmvm/libwasmvm/Cargo.toml"
cargo test --release --locked --manifest-path "$vm_build_dir/wasmvm/libwasmvm/Cargo.toml" native_async_categories
cargo test --release --locked --manifest-path "$vm_build_dir/cosmwasm-2.2.2/packages/vm/Cargo.toml" --lib native_async
case "$(uname -s)" in
 Darwin) vm_library=libwasmvm.dylib; vm_link_library=$vm_library ;;
 Linux)
  vm_library=libwasmvm.so
  case "$(uname -m)" in
   aarch64) vm_link_library=libwasmvm.aarch64.so ;;
   x86_64) vm_link_library=libwasmvm.x86_64.so ;;
   *) echo 'Unsupported Linux architecture' >&2; exit 1 ;;
  esac ;;
 *) echo 'Unsupported native build platform' >&2; exit 1 ;;
esac
cp "$vm_build_dir/wasmvm/libwasmvm/bindings.h" "$vm_build_dir/wasmvm/internal/api/bindings.h"
# Negative identity test: patched Go + stock library must fail to link on the
# additive capability symbol, rather than silently erasing error categories.
if (cd "$vm_build_dir/wasmvm" && GOWORK=off go test -run '^$' ./internal/api) >"$vm_build_dir/stock-link.log" 2>&1; then
 echo 'ERROR: stock native library unexpectedly passed capability link gate' >&2; exit 1
fi
rg -q 'native_async_error_abi' "$vm_build_dir/stock-link.log" || { cat "$vm_build_dir/stock-link.log"; exit 1; }
cp "$CARGO_TARGET_DIR/release/$vm_library" "$vm_build_dir/wasmvm/internal/api/$vm_link_library"
export NATIVE_ASYNC_VM="$vm_build_dir/wasmvm"
printf 'Native provenance ABI1 library: '
shasum -a 256 "$NATIVE_ASYNC_VM/internal/api/$vm_link_library"
