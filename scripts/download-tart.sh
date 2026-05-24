#!/usr/bin/env bash
# Download the latest Tart release and place it inside the Tauri resources
# directory so it can be bundled with the app.
#
# Tart is distributed as a full macOS .app bundle. Its `com.apple.security
# .virtualization` entitlement is granted by an embedded provisioning
# profile, so we MUST keep the .app bundle structure intact — extracting
# the inner Mach-O binary breaks signing and macOS will SIGKILL it.
#
# Layout after this script runs:
#   src-tauri/resources/tart/tart.app/...                  (the .app bundle)
#   src-tauri/resources/third-party/tart/LICENSE           (Apache 2.0)
#
# Tart is Apache-2.0 (https://github.com/cirruslabs/tart).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RESOURCE_DIR="$REPO_ROOT/src-tauri/resources/tart"
LICENSE_DIR="$REPO_ROOT/src-tauri/resources/third-party/tart"

mkdir -p "$RESOURCE_DIR" "$LICENSE_DIR"

# Resolve latest release tag.
LATEST_TAG="$(curl -fsSL https://api.github.com/repos/cirruslabs/tart/releases/latest \
  | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)"
if [ -z "$LATEST_TAG" ]; then
  echo "Failed to resolve latest tart release tag" >&2
  exit 1
fi

ASSET_URL="https://github.com/cirruslabs/tart/releases/download/$LATEST_TAG/tart.tar.gz"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Downloading tart $LATEST_TAG …"
curl -fsSL -o "$TMP/tart.tar.gz" "$ASSET_URL"

# Wipe any previous tart.app before extracting so deletions in upstream are
# reflected (no stale files left behind).
rm -rf "$RESOURCE_DIR/tart.app"
tar -xzf "$TMP/tart.tar.gz" -C "$RESOURCE_DIR"

if [ ! -x "$RESOURCE_DIR/tart.app/Contents/MacOS/tart" ]; then
  echo "Extracted archive does not contain tart.app/Contents/MacOS/tart" >&2
  exit 1
fi

# License (Apache-2.0). Tart's repo currently keeps only LICENSE; NOTICE is
# optional under Apache-2.0 so we don't fail if missing.
if [ -f "$RESOURCE_DIR/LICENSE" ]; then
  mv "$RESOURCE_DIR/LICENSE" "$LICENSE_DIR/LICENSE"
fi

echo "Installed tart $LATEST_TAG to $RESOURCE_DIR/tart.app"
"$RESOURCE_DIR/tart.app/Contents/MacOS/tart" --version
