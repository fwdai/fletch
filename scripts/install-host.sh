#!/usr/bin/env bash
#
# One-line installer for fletch-host, the headless engine
# (crates/fletch-host/README.md): download this machine's release tarball,
# verify it the way `fletch-host update` does, install the binary, start the
# service, end at the pairing QR.
#
#   curl -fsSL https://raw.githubusercontent.com/fwdai/fletch/main/scripts/install-host.sh | bash
#
# This script bootstraps once; upgrades belong to the binary it installed. A
# re-run finds that binary and hands over to `fletch-host update`, which
# verifies the same way, copies the database aside and restarts the service
# through the unit it already has — rather than re-running `service install`,
# which would rewrite that unit from this script's (absent) --port/--name/
# --system flags and reset the operator's service configuration.
#
#   FLETCH_HOST_VERSION      pin a release, e.g. 0.8.0 (default: the latest)
#   FLETCH_HOST_INSTALL_DIR  where the binary goes (default: ~/.local/bin)
#   FLETCH_HOST_SKIP_SIGNATURE=1  install without verifying the release
#                            signature — insecure, and the one way past the
#                            minisign requirement below
set -euo pipefail

REPO="fwdai/fletch"
# `plugins.updater.pubkey` from src-tauri/tauri.conf.json — base64 of a minisign
# public key file, the same constant `update::UPDATER_PUBKEY` carries.
PUBKEY="dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDY2Njg3OUNDQ0E2MUYxNDUKUldSRjhXSEt6SGxvWnZTdnplUDEzRHVSdmR2OEJwanhLTEJLQ2MzTnVLU0tZVTBnVElzbjV1Vk0K"
INSTALL_DIR="${FLETCH_HOST_INSTALL_DIR:-$HOME/.local/bin}"

die() {
  echo "install-host: $*" >&2
  exit 1
}

# `exec` never runs bash's EXIT trap, so the temp dir has to go by hand before
# every one of them — and the trap has to stop firing for a directory that is
# already gone.
cleanup() {
  trap - EXIT
  if [ -n "${TMP:-}" ]; then
    rm -rf "$TMP"
    TMP=
  fi
}

# How to get minisign on this machine, for the message that refuses to install
# without it.
minisign_hint() {
  case "$(uname -s)" in
    Darwin) echo "brew install minisign" ;;
    Linux)
      if command -v apt >/dev/null 2>&1; then
        echo "sudo apt install minisign"
      elif command -v dnf >/dev/null 2>&1; then
        echo "sudo dnf install minisign"
      else
        echo "your package manager's minisign package"
      fi
      ;;
    *) echo "your package manager's minisign package" ;;
  esac
}

# A fletch-host already on this machine: the install dir's first, since that is
# the one this script would replace, then whatever is on PATH.
installed_host() {
  if [ -x "$INSTALL_DIR/fletch-host" ]; then
    echo "$INSTALL_DIR/fletch-host"
  else
    command -v fletch-host 2>/dev/null || true
  fi
}

target() {
  local os machine
  os="$(uname -s)"
  machine="$(uname -m)"
  case "$os/$machine" in
    Linux/x86_64) echo x86_64-unknown-linux-gnu ;;
    Linux/aarch64 | Linux/arm64) echo aarch64-unknown-linux-gnu ;;
    Darwin/arm64) echo aarch64-apple-darwin ;;
    *) die "no fletch-host release for $os $machine.
Releases cover Linux x86-64, Linux arm64 and Apple silicon; build from a
checkout instead (crates/fletch-host/README.md)." ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    die "neither sha256sum nor shasum is on PATH; cannot verify the download"
  fi
}

# `base64 -d` is GNU's spelling, `-D` is BSD's.
b64_decode() {
  base64 -d <"$1" >"$2" 2>/dev/null || base64 -D <"$1" >"$2"
}

latest_version() {
  curl -fsSL -H 'User-Agent: fletch-host-install' \
    "https://api.github.com/repos/$REPO/releases/latest" |
    sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\{0,1\}\([^"]*\)".*/\1/p' |
    head -n 1
}

# Verify the downloaded tarball against the release key, and refuse to install
# anything unverified. A .sha256 served from the same place as the tarball only
# proves the download arrived intact, so the signature is the whole of the trust
# here — a missing minisign or a missing .sig is a refusal, exactly as it is in
# `fletch-host update`. `FLETCH_HOST_SKIP_SIGNATURE` is the one way past it, and
# it says so loudly.
verify_signature() {
  local dir="$1" version="$2" name="$3" base="$4"

  if [ -n "${FLETCH_HOST_SKIP_SIGNATURE:-}" ]; then
    echo "WARNING: FLETCH_HOST_SKIP_SIGNATURE is set, so the release signature is NOT"
    echo "WARNING: checked and nothing proves this binary is the project's. Insecure."
    return 0
  fi

  command -v minisign >/dev/null 2>&1 ||
    die "minisign is not on PATH, and it is what proves this binary is the project's
rather than whoever answered the download. Install it with
    $(minisign_hint)
and run this again — or set FLETCH_HOST_SKIP_SIGNATURE=1 to install without the
check, which is insecure."
  curl -fsSL -o "$dir/$name.sig.b64" "$base.sig" ||
    die "$version published $name without its .sig; refusing"

  # Both the key and the .sig are stored base64-encoded (that is what
  # `tauri signer` writes); minisign(1) wants the decoded files.
  printf '%s' "$PUBKEY" >"$dir/pubkey.b64"
  b64_decode "$dir/pubkey.b64" "$dir/fletch.pub"
  b64_decode "$dir/$name.sig.b64" "$dir/$name.sig"
  minisign -V -p "$dir/fletch.pub" -x "$dir/$name.sig" -m "$dir/$name" >/dev/null ||
    die "the download is not signed by the Fletch release key; nothing was installed"
  echo "signature ok (minisign, the desktop updater's key)"
}

# Download the tarball, its .sha256 and its .sig into $1, verify both, and
# leave the extracted `fletch-host` at $1/fletch-host.
fetch_and_verify() {
  local dir="$1" version="$2" tgt="$3" name base
  name="fletch-host-$version-$tgt.tar.gz"
  base="https://github.com/$REPO/releases/download/v$version/$name"

  echo "downloading $name"
  curl -fsSL -o "$dir/$name" "$base" ||
    die "cannot download $base
No such release, or it published no archive for $tgt."
  curl -fsSL -o "$dir/$name.sha256" "$base.sha256" ||
    die "$version published $name without its .sha256; refusing"

  local expected actual
  expected="$(cut -d' ' -f1 <"$dir/$name.sha256")"
  actual="$(sha256_of "$dir/$name")"
  [ "$expected" = "$actual" ] ||
    die "the download does not match its checksum (expected $expected, got $actual); nothing was installed"
  echo "sha256 ok"

  verify_signature "$dir" "$version" "$name" "$base"

  tar xzf "$dir/$name" -C "$dir"
  [ -f "$dir/fletch-host" ] || die "$name holds no fletch-host binary"
}

# Everything runs from main so that `curl | bash` parses the whole script
# before executing any of it; a truncated download then does nothing.
main() {
  VERSION="${FLETCH_HOST_VERSION:-}"
  VERSION="${VERSION#v}"

  # Already installed? Then this is an upgrade, and the installed binary owns
  # it: `update` verifies the sha256 and the signature fail-closed, copies the
  # database aside, swaps the binary and restarts through the unit it already
  # has. Re-running `service install` from here would rewrite that unit from
  # flags this script does not have, silently resetting an operator's --port,
  # --name or --system configuration.
  EXISTING="$(installed_host)"
  if [ -n "$EXISTING" ]; then
    echo "fletch-host is already installed at $EXISTING; handing the upgrade to \`fletch-host update\`."
    cleanup
    if [ -n "$VERSION" ]; then
      exec "$EXISTING" update "$VERSION"
    fi
    exec "$EXISTING" update
  fi

  TARGET="$(target)"
  if [ -z "$VERSION" ]; then
    VERSION="$(latest_version)" || true
    [ -n "$VERSION" ] || die "cannot read the latest release from the GitHub API; set FLETCH_HOST_VERSION to pin one"
  fi
  echo "fletch-host $VERSION ($TARGET) -> $INSTALL_DIR"

  TMP="$(mktemp -d)"
  trap cleanup EXIT
  fetch_and_verify "$TMP" "$VERSION" "$TARGET"

  BIN="$INSTALL_DIR/fletch-host"
  mkdir -p "$INSTALL_DIR"
  # Install beside the target and rename: a running host's binary cannot be
  # written in place on Linux, but it can be replaced.
  install -m 755 "$TMP/fletch-host" "$INSTALL_DIR/.fletch-host.new"
  mv -f "$INSTALL_DIR/.fletch-host.new" "$BIN"
  echo "installed $VERSION at $BIN"

  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) echo "note: $INSTALL_DIR is not on your PATH — add it with
    export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
  esac

  if [ -n "${FLETCH_HOST_SKIP_SERVICE:-}" ]; then
    echo "FLETCH_HOST_SKIP_SERVICE is set; stopping before \`service install\`"
    exit 0
  fi

  "$BIN" service install

  if [ "$(uname -s)" = "Linux" ] && command -v loginctl >/dev/null 2>&1; then
    WHO="${USER:-$(id -un)}"
    if [ "$(loginctl show-user "$WHO" --property=Linger 2>/dev/null)" != "Linger=yes" ]; then
      echo
      echo "A systemd user unit stops when you log out. Lingering needs sudo, so run it yourself:"
      echo "    sudo loginctl enable-linger $WHO"
    fi
  fi

  echo
  printf 'waiting for the host to answer'
  # Wall clock, not a count of the sleeps: `status` takes its own time (it has
  # its own timeout on the admin socket), so counting sleeps would promise 30s
  # and wait much longer.
  DEADLINE=$((SECONDS + 30))
  until "$BIN" status >/dev/null 2>&1; do
    if [ "$SECONDS" -ge "$DEADLINE" ]; then
      echo
      die "the host did not answer its admin socket in 30s; check the logs \`service install\` printed above"
    fi
    printf '.'
    sleep 1
  done
  printf ' ok\n\n'

  cleanup
  exec "$BIN" pair
}

main "$@"
