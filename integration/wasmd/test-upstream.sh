#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
adapter_dir="$PWD"
export GOTOOLCHAIN=go1.23.6
export GOPATH="${NATIVE_ASYNC_GOPATH:-$(mktemp -d /private/tmp/native-async-go.XXXXXX)}"
export GOCACHE="${NATIVE_ASYNC_GOCACHE:-$GOPATH/build}"
go mod download
source ./build-vm.sh
upstream_dir="$(mktemp -d /private/tmp/native-async-wasmd.XXXXXX)"
source_dir="$(go list -m -f '{{.Dir}}' github.com/CosmWasm/wasmd)"
cp -R "$source_dir/." "$upstream_dir/"
chmod -R u+w "$upstream_dir"
go run ./cmd/patch-upstream "$upstream_dir"
cd "$upstream_dir"
go mod edit -require=cosmwasm-durable-async/integration/wasmd@v0.0.0 "-replace=cosmwasm-durable-async/integration/wasmd=$adapter_dir" "-replace=github.com/CosmWasm/wasmvm/v2=$NATIVE_ASYNC_VM"
go test -mod=mod -run '^$' ./app
workspace_dir="$(mktemp -d /private/tmp/native-async-workspace.XXXXXX)"
cd "$workspace_dir"
go work init "$adapter_dir" "$upstream_dir" "$NATIVE_ASYNC_VM"
export GOWORK="$workspace_dir/go.work"
cd "$adapter_dir"
go test -tags upstream -count=1 ./...
printf 'Patched wasmd checkout: %s\n' "$upstream_dir"
printf 'Patched VM checkout: %s\nGOWORK=%s\n' "$NATIVE_ASYNC_VM" "$GOWORK"
