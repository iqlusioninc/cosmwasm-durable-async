#!/usr/bin/env bash
# Fetch a checksum-pinned local tool; never modifies the user's global PATH.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) platform=arm64-macos; sha=74428be348c1a09863e7b642a1fa948cabf8ec9561052233d8288e941951725b ;;
  Darwin-x86_64) platform=x86_64-macos; sha=cc18b14d2b673d9c66bf54f31ff2b0ceb23ba5132455b893965ae2792f9e00dd ;;
  Linux-x86_64) platform=x86_64-linux; sha=e959f2170af4c20c552e9de3a0253704d6a9d2766e8fdb88e4d6ac4bae9388fe ;;
  Linux-aarch64|Linux-arm64) platform=aarch64-linux; sha=4b6bd61ba6cd3b18c993b4657d93426c782f9b91b74be0d38018cd8be1319376 ;;
  *) echo 'Unsupported Binaryen platform' >&2; exit 1 ;;
esac
mkdir -p "$root/target/tools"
archive="$root/target/tools/binaryen-version_123-$platform.tar.gz"
curl --fail --location --silent --show-error "https://github.com/WebAssembly/binaryen/releases/download/version_123/binaryen-version_123-$platform.tar.gz" -o "$archive"
python3 - "$archive" "$sha" <<'PY'
import hashlib, pathlib, sys
actual = hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest()
if actual != sys.argv[2]:
    raise SystemExit(f"Binaryen checksum mismatch: {actual}")
PY
tar -xzf "$archive" -C "$root/target/tools"
"$root/target/tools/binaryen-version_123/bin/wasm-opt" --version
