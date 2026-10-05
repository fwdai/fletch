#!/usr/bin/env bash
#
# Regenerate the app-icon rasters from the two 1024x1024 masters with
# `tauri icon`, keeping only the files the bundles actually reference:
#
#   src-tauri/icons/icon.png          desktop master: dark rounded square with
#                                     drop shadow and transparent margin
#                                     (Apple's 824/1024 inset)
#   mobile/src-tauri/icons/icon.png   full-bleed variant of the same tile for
#                                     iOS, which masks corners itself and
#                                     rejects transparency
#
# Each run writes 32x32, 128x128, 128x128@2x and icon.icns next to the master
# (tauri.conf.json `bundle.icon`), and the ios/ AppIcon set for mobile. The
# Windows/Android output `tauri icon` also emits is left in the temp dir.
#
# The macOS menu-bar template icon is separate: scripts/gen_tray_icon.py.
#
# Run: scripts/gen-app-icons.sh

set -euo pipefail
cd "$(dirname "$0")/.."

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

bundle_pngs=(32x32.png 128x128.png 128x128@2x.png icon.icns)

gen() {
  local icons=$1 out="$tmp/$2"
  # `tauri icon` lists every file it creates; `set -e` surfaces real failures.
  bun tauri icon "$icons/icon.png" -o "$out" >/dev/null 2>&1
  for f in "${bundle_pngs[@]}"; do cp "$out/$f" "$icons/$f"; done
  echo "wrote ${bundle_pngs[*]} in $icons"
}

gen src-tauri/icons desktop
gen mobile/src-tauri/icons mobile
cp "$tmp"/mobile/ios/AppIcon-*.png mobile/src-tauri/icons/ios/
echo "wrote ios/AppIcon-*.png in mobile/src-tauri/icons"
