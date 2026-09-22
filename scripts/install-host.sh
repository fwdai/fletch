#!/usr/bin/env bash
#
# One-line installer for fletch-host, the headless engine
# (crates/fletch-host/README.md): download this machine's release tarball,
# verify it the way `fletch-host update` does, install the binary, start the
# service, end at the pairing QR. Safe to re-run — that is how you upgrade.
#
#   curl -fsSL https://raw.githubusercontent.com/fwdai/fletch/main/scripts/install-host.sh | bash
#
#   FLETCH_HOST_VERSION      pin a release, e.g. 0.8.0 (default: the latest)
#   FLETCH_HOST_INSTALL_DIR  where the binary goes (default: ~/.local/bin)
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

  if command -v minisign >/dev/null 2>&1 &&
    curl -fsSL -o "$dir/$name.sig.b64" "$base.sig" 2>/dev/null; then
    # Both the key and the .sig are stored base64-encoded (that is what
    # `tauri signer` writes); minisign(1) wants the decoded files.
    printf '%s' "$PUBKEY" >"$dir/pubkey.b64"
    b64_decode "$dir/pubkey.b64" "$dir/fletch.pub"
    b64_decode "$dir/$name.sig.b64" "$dir/$name.sig"
    minisign -V -p "$dir/fletch.pub" -x "$dir/$name.sig" -m "$dir/$name" >/dev/null ||
      die "the download is not signed by the Fletch release key; nothing was installed"
    echo "signature ok (minisign, the desktop updater's key)"
  else
    echo "note: the release signature was not checked (minisign on PATH verifies it)"
  fi

  tar xzf "$dir/$name" -C "$dir"
  [ -f "$dir/fletch-host" ] || die "$name holds no fletch-host binary"
}

TARGET="$(target)"
VERSION="${FLETCH_HOST_VERSION:-}"
VERSION="${VERSION#v}"
if [ -z "$VERSION" ]; then
  VERSION="$(latest_version)" || true
  [ -n "$VERSION" ] || die "cannot read the latest release from the GitHub API; set FLETCH_HOST_VERSION to pin one"
fi
echo "fletch-host $VERSION ($TARGET) -> $INSTALL_DIR"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
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
WAITED=0
until "$BIN" status >/dev/null 2>&1; do
  if [ "$WAITED" -ge 30 ]; then
    echo
    die "the host did not answer its admin socket in 30s; check the logs \`service install\` printed above"
  fi
  printf '.'
  sleep 1
  WAITED=$((WAITED + 1))
done
printf ' ok\n\n'

exec "$BIN" pair
