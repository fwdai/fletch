#!/bin/sh
# `tauriScript` for tauri-action in release.yml. The action always runs
# `<tauriScript> build <args>`; the release workflow compiles the app per
# architecture in parallel jobs and joins the binaries with lipo itself, so
# here `build` becomes `bundle`: package, sign and notarize what is already in
# src-tauri/target, without compiling again. The action then finds the bundles,
# publishes them and writes latest.json exactly as after a real build.
set -eu
[ "${1:-}" = build ] || { echo "tauri-bundle-prebuilt.sh: expected 'build', got '${1:-}'" >&2; exit 1; }
shift
exec bun run tauri bundle "$@"
