#!/usr/bin/env bash
# Interactive base-image bake. Use this when the in-app bake fails because
# of cirruslabs/ubuntu's flaky first-boot networking.
#
# What this does:
#   1. Cleans up any leftover bake artifacts.
#   2. Clones the upstream Ubuntu image into a builder VM.
#   3. Opens the Tart GUI window so YOU can log in and paste the setup
#      block manually (this is the part that doesn't work over the network).
#   4. Waits for you to `sudo shutdown -h now` from inside the VM.
#   5. Renames the result to `base-dev`, ready for the algiers app.
#
# Net result: ~10 minutes of babysitting one VM window, end up with a
# working base image the app can clone agents from.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

TART="./src-tauri/resources/tart/tart.app/Contents/MacOS/tart"
UPSTREAM="ghcr.io/cirruslabs/ubuntu:24.04"
BUILDER_NAME="algiers-base-builder"
FINAL_NAME="base-dev"
APP_DATA="$HOME/Library/Application Support/com.algiers.app"
PUBKEY_PATH="$APP_DATA/id_ed25519_algiers.pub"

# ---------- preflight ----------

if [ ! -x "$TART" ]; then
  echo "ERROR: bundled tart not found at $TART" >&2
  echo "Did 'npm install' run? Try: bash scripts/download-tart.sh" >&2
  exit 1
fi

if [ ! -f "$PUBKEY_PATH" ]; then
  echo "ERROR: algiers SSH public key not found at:" >&2
  echo "  $PUBKEY_PATH" >&2
  echo "Run the algiers app at least once so it generates one." >&2
  exit 1
fi

PUBKEY=$(<"$PUBKEY_PATH")

# ---------- clean previous attempts ----------

echo "[1/4] Cleaning up any previous attempts…"
"$TART" stop "$BUILDER_NAME"  >/dev/null 2>&1 || true
"$TART" delete "$BUILDER_NAME" >/dev/null 2>&1 || true
# Don't delete base-dev unconditionally — only if user opts in.
if "$TART" list --quiet 2>/dev/null | grep -qx "$FINAL_NAME"; then
  read -r -p "  base-dev already exists. Delete it and start over? [y/N] " ans
  case "${ans:-N}" in
    y|Y|yes|YES)
      "$TART" stop "$FINAL_NAME"  >/dev/null 2>&1 || true
      "$TART" delete "$FINAL_NAME" >/dev/null 2>&1
      echo "  Deleted existing base-dev."
      ;;
    *)
      echo "  Aborting — keeping existing base-dev. (Quit the algiers app and re-launch; it should detect it.)"
      exit 0
      ;;
  esac
fi

# ---------- pull upstream ----------

echo "[2/4] Pulling $UPSTREAM (this is the 1–2 GB download)…"
"$TART" clone "$UPSTREAM" "$BUILDER_NAME"

# ---------- the manual part ----------

SETUP_BLOCK=$(cat <<SETUP
# ===== Paste this entire block into the VM as the admin user =====
set -e
echo '>>> Disabling firewall'
sudo ufw --force disable
sudo systemctl stop ufw 2>/dev/null || true
sudo systemctl mask ufw 2>/dev/null || true

echo '>>> Updating apt'
sudo apt-get update -y -qq

echo '>>> Installing core packages'
sudo apt-get install -y -qq curl git ca-certificates build-essential

echo '>>> Installing Node.js 20'
curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash - >/dev/null
sudo apt-get install -y -qq nodejs

echo '>>> Installing Claude Code CLI'
sudo npm install -g @anthropic-ai/claude-code

echo '>>> Adding host public key + sudoers'
mkdir -p ~/.ssh && chmod 700 ~/.ssh
echo '$PUBKEY' >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys
echo 'admin ALL=(ALL) NOPASSWD: /usr/bin/mount, /usr/bin/umount' | sudo tee /etc/sudoers.d/algiers >/dev/null
sudo chmod 440 /etc/sudoers.d/algiers

echo '>>> Pre-creating /workspace'
sudo mkdir -p /workspace
sudo chown admin:admin /workspace

echo '>>> Disabling cloud-init for clones'
sudo touch /etc/cloud/cloud-init.disabled
sudo systemctl mask cloud-init.service cloud-init-local.service cloud-config.service cloud-final.service 2>/dev/null || true

echo
echo 'BAKE COMPLETE — shutting down in 3 seconds. The Tart window will close.'
sleep 3
sudo shutdown -h now
# ================================================================
SETUP
)

cat <<EOF

================================================================
[3/4] MANUAL STEP — please read carefully
================================================================

A Tart GUI window is about to open. Follow these steps:

  1. WAIT ~1–2 min for boot → you'll see the Ubuntu login prompt.
  2. Log in:    admin / admin
  3. COPY the block below (entire thing, including the comments).
  4. PASTE it into the VM window (right-click usually works; or
     Cmd-V if Tart's clipboard sharing is on).
  5. Wait for "BAKE COMPLETE" message (~3–5 min for apt + npm).
  6. The VM auto-shuts down. The Tart window closes by itself.

If something goes wrong, just close the Tart window and re-run
this script — it'll start fresh.

================================================================
COPY THIS BLOCK:
================================================================
$SETUP_BLOCK
================================================================

EOF

read -r -p "Press ENTER to open the VM window…"

# Open the GUI (NOT --no-graphics this time — the whole point is to see it)
"$TART" run "$BUILDER_NAME" &
TART_PID=$!

echo
echo "VM window opened (tart PID: $TART_PID)."
echo "Once you trigger 'shutdown -h now' inside, this script will continue."
echo

# Wait for tart run to exit, indicating the guest powered off.
wait "$TART_PID" || true

# ---------- finalize ----------

echo
echo "[4/4] Finalizing as $FINAL_NAME…"
# Tart has no `rename`; clone + delete is the standard pattern.
"$TART" clone "$BUILDER_NAME" "$FINAL_NAME"
"$TART" delete "$BUILDER_NAME"

cat <<EOF

================================================================
DONE.

  • Image:    $FINAL_NAME
  • Storage:  $("$TART" list --quiet | grep -x "$FINAL_NAME" >/dev/null && echo "registered with Tart" || echo "MISSING — something went wrong")

Now in the algiers app:
  • Refresh the window (Cmd-R) or restart the dev server.
  • The "missing base image" banner should be gone.
  • "+ Spawn" is enabled. Click it.

Want to share this image with other machines / teammates?
  $TART login ghcr.io  # authenticate with a GitHub PAT (write:packages)
  $TART push ghcr.io/<your-account>/algiers-base:latest $FINAL_NAME

================================================================
EOF
