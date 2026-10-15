#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
adapter_dir="$PWD"
# Each run builds about 2 GB of patched VM, Cargo home, wasmd copy and Go
# module cache. Remove them on exit unless NATIVE_ASYNC_KEEP=1 is set (for
# example, to rerun targeted tests against the printed GOWORK).
cleanup_dirs=()
cleanup() {
  [[ "${NATIVE_ASYNC_KEEP:-0}" == 1 ]] && return
  for d in ${vm_build_dir:+"$vm_build_dir"} ${cleanup_dirs[@]+"${cleanup_dirs[@]}"}; do
    chmod -R u+w "$d" 2>/dev/null || true
    rm -rf "$d"
  done
}
trap cleanup EXIT
export GOTOOLCHAIN=go1.23.6
if [[ -z "${NATIVE_ASYNC_GOPATH:-}" ]]; then
  NATIVE_ASYNC_GOPATH="$(mktemp -d /private/tmp/native-async-go.XXXXXX)"
  cleanup_dirs+=("$NATIVE_ASYNC_GOPATH")
fi
export GOPATH="$NATIVE_ASYNC_GOPATH"
export GOCACHE="${NATIVE_ASYNC_GOCACHE:-$GOPATH/build}"
go mod download
source ./build-vm.sh
upstream_dir="$(mktemp -d /private/tmp/native-async-wasmd.XXXXXX)"
cleanup_dirs+=("$upstream_dir")
source_dir="$(go list -m -f '{{.Dir}}' github.com/CosmWasm/wasmd)"
cp -R "$source_dir/." "$upstream_dir/"
chmod -R u+w "$upstream_dir"
go run ./cmd/patch-upstream "$upstream_dir"
cd "$upstream_dir"
go mod edit -require=cosmwasm-durable-async/integration/wasmd@v0.0.0 "-replace=cosmwasm-durable-async/integration/wasmd=$adapter_dir" "-replace=github.com/CosmWasm/wasmvm/v2=$NATIVE_ASYNC_VM"
go test -mod=mod -run '^$' ./app
workspace_dir="$(mktemp -d /private/tmp/native-async-workspace.XXXXXX)"
cleanup_dirs+=("$workspace_dir")
cd "$workspace_dir"
go work init "$adapter_dir" "$upstream_dir" "$NATIVE_ASYNC_VM"
export GOWORK="$workspace_dir/go.work"
cd "$adapter_dir"
if [[ -n "${NATIVE_ASYNC_TEST_RUN:-}" ]]; then
  go test -tags upstream -count=1 -run "$NATIVE_ASYNC_TEST_RUN" ./...
else
  go test -tags upstream -count=1 ./...
fi
if [[ "${NATIVE_ASYNC_KEEP:-0}" == 1 ]]; then
  printf 'Patched wasmd checkout: %s\n' "$upstream_dir"
  printf 'Patched VM checkout: %s\nGOWORK=%s\n' "$NATIVE_ASYNC_VM" "$GOWORK"
fi
