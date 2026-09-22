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
# re-run with the same FLETCH_HOST_INSTALL_DIR finds that binary — that
# directory's, never another one on PATH — and hands over to `fletch-host
# update`, which
# verifies the same way, copies the database aside and restarts the service
# through the unit it already has — rather than re-running `service install`,
# which would rewrite that unit from this script's (absent) --port/--name/
# --system flags and reset the operator's service configuration.
#
# A re-run with a *different* FLETCH_HOST_INSTALL_DIR finds no binary there and
# would otherwise be a fresh install, `service install` and all. So the service
# itself is looked for too, through `fletch-host service show`, and an install
# that would rewrite someone else's unit stops instead.
#
#   FLETCH_HOST_VERSION      pin a release, e.g. 0.8.0 (default: the latest)
#   FLETCH_HOST_INSTALL_DIR  where the binary goes (default: ~/.local/bin)
#   FLETCH_HOST_SKIP_SERVICE=1  install the binary and stop before
#                            `service install`, leaving any installed service
#                            as it is
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

# The fletch-host this script would replace: the one in $INSTALL_DIR, and only
# that one. Deliberately not "whatever is on PATH" — the service unit names an
# absolute path, so a PATH hit can be a different binary in a different
# directory, and handing the upgrade to it would update something the service
# never runs while leaving the service's own binary untouched.
installed_host() {
  if [ -x "$INSTALL_DIR/fletch-host" ]; then
    echo "$INSTALL_DIR/fletch-host"
  fi
}

# One `key=value` line out of `fletch-host service show`.
show_field() {
  printf '%s\n' "$1" | sed -n "s/^$2=//p" | head -n 1
}

# Refuse to reset a service this script did not configure. $1 is the verified
# binary to ask — the one in $TMP, before anything has been installed.
#
# A fresh install ends in a flag-less `fletch-host service install`, which
# overwrites whatever unit is there and restarts it with default arguments.
# Getting this far with a unit already installed means $INSTALL_DIR holds no
# binary: a re-run pointed at another FLETCH_HOST_INSTALL_DIR, or a binary
# deleted by hand. Rewriting the unit would then drop the --data-dir, --port,
# --name or --system it was installed with and restart the host, possibly
# against a different database. This script has none of those flags and does
# not try to guess them, so the rule is: refuse whenever a unit exists, except
# in the one case where the rewrite would change nothing — same binary path,
# the arguments a flag-less install writes anyway, and not a --system unit
# (which lives elsewhere, so a flag-less install would leave it running and
# start a second host beside it).
refuse_to_reset_service() {
  local show
  # A non-zero exit means no service is installed, which is the ordinary case.
  show="$("$1" service show 2>/dev/null)" || return 0
  decide_service_conflict "$show"
}

# The decision, over `service show`'s output alone.
decide_service_conflict() {
  local unit unit_exec args default_args
  unit="$(show_field "$1" unit)"
  unit_exec="$(show_field "$1" exec)"
  args="$(show_field "$1" args)"
  default_args="$(show_field "$1" default_args)"

  if [ "$unit_exec" != "$INSTALL_DIR/fletch-host" ]; then
    die "a fletch-host service is already installed, and it runs a binary this
install would not touch:
    unit: $unit
    runs: $unit_exec
Installing into $INSTALL_DIR would rewrite that unit with default arguments and
restart the host, possibly against a different database. Upgrade the host that
is installed, in place:
    curl -fsSL https://raw.githubusercontent.com/$REPO/main/scripts/install-host.sh | FLETCH_HOST_INSTALL_DIR=$(dirname "$unit_exec") bash
or remove its service first:
    $unit_exec service uninstall"
  fi

  # A --system unit is not the unit a flag-less `service install` writes: that
  # one is a user unit, so this would start a second host beside the first.
  case "$unit" in
    /etc/systemd/system/*)
      die "a fletch-host service is already installed system-wide at
    $unit
but $INSTALL_DIR/fletch-host is gone, so this is a fresh install, and its
flag-less \`service install\` writes a *user* unit — leaving that one running
and starting a second host beside it.
Re-run with FLETCH_HOST_SKIP_SERVICE=1 to put the binary back and leave the
service alone, or remove the unit yourself to start over." ;;
  esac

  if [ "$args" != "$default_args" ]; then
    die "a fletch-host service is already installed at
    $unit
but $INSTALL_DIR/fletch-host is gone, so this is a fresh install, and its
flag-less \`service install\` would rewrite that unit as
    fletch-host $default_args
losing how it was configured:
    fletch-host $args
Re-run with FLETCH_HOST_SKIP_SERVICE=1 to put the binary back and leave the
service alone, or remove the unit yourself to start over."
  fi

  echo "note: the service at $unit runs $unit_exec with this script's own default"
  echo "note: arguments, so \`service install\` writes back an equivalent unit."
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

  # The binary in $TMP is verified now, and it is the one that knows where
  # units live on this machine — so ask it about the service before writing
  # anything. Skipped when `service install` is not going to run at all.
  if [ -z "${FLETCH_HOST_SKIP_SERVICE:-}" ]; then
    refuse_to_reset_service "$TMP/fletch-host"
  fi

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
  # Wall clock, not a count of the sleeps: `status` takes its own time (a host
  # that is still booting answers "the host is still starting" as soon as it is
  # asked, but the ask itself is not free), so counting sleeps would promise 30s
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
