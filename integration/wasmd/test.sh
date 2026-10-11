#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
export GOTOOLCHAIN=go1.23.6
export GOPATH="${NATIVE_ASYNC_GOPATH:-$(mktemp -d /private/tmp/native-async-go.XXXXXX)}"
export GOCACHE="${NATIVE_ASYNC_GOCACHE:-$GOPATH/build}"
go mod download
go test -count=1 -v ./...
