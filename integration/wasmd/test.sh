#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
# Both keeper and app suites require the same typed native boundary.
exec bash ./test-upstream.sh
