#!/usr/bin/env bash
#
# One product version, four files. Prints it on stdout, or explains the
# disagreement on stderr and exits 1.
#
# `src-tauri/tauri.conf.json` is the authority: the release tag, the release
# name and the `fletch-host` archive names are all derived from it. The three
# Cargo manifests have to match because each one is what some binary *reports*:
# the desktop's `app.package_info().version`, `fletch-host --version` /
# `fletch-host status`, and `HostInfo.appVersion` (from `fletch-core`'s
# `CARGO_PKG_VERSION`) — a crate left behind reports a version no release ever
# had, which is what happened to the first `fletch-host` builds (0.1.0).
#
# Run by CI on every push and PR, by the release workflow before it packages,
# and by you before you open the bump PR. `package.json` is deliberately not
# here: it feeds nothing a user can read a version off, and npm's own tooling
# rewrites it.
#
# See docs/RELEASING.md.

set -euo pipefail

cd "$(dirname "$0")/.."

version="$(jq -r .version src-tauri/tauri.conf.json)"
if [[ -z "$version" || "$version" == "null" ]]; then
  echo "error: no version in src-tauri/tauri.conf.json" >&2
  exit 1
fi

status=0
for manifest in src-tauri/Cargo.toml crates/fletch-core/Cargo.toml crates/fletch-host/Cargo.toml; do
  # The first `version = "…"` after `[package]`, so a dependency's version
  # cannot be mistaken for the crate's own.
  found="$(awk '/^\[package\]/{p=1;next} /^\[/{p=0} p&&/^version *=/{gsub(/^version *= *"|"$/,"");print;exit}' "$manifest")"
  if [[ "$found" != "$version" ]]; then
    echo "error: $manifest is $found, but src-tauri/tauri.conf.json is $version" >&2
    status=1
  fi
done

if [[ $status -ne 0 ]]; then
  echo "All four must agree; see docs/RELEASING.md." >&2
  exit $status
fi

echo "$version"
