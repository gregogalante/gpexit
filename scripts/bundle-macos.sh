#!/bin/sh
set -eu
if [ "$(uname -s)" != Darwin ]; then
  echo 'This bundler requires macOS.' >&2
  exit 1
fi
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_dir"
if ! cargo tauri --version >/dev/null 2>&1; then
  echo 'Install packaging CLI: cargo install tauri-cli --version 2.12.4 --locked' >&2
  exit 1
fi
cargo tauri build --ci --bundles app -- --locked
codesign --verify --deep --strict "$project_dir/target/release/bundle/macos/GPExit.app"
printf '%s\n' "$project_dir/target/release/bundle/macos/GPExit.app"
