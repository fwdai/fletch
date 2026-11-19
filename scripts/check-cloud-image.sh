#!/usr/bin/env bash
#
# The cloud image's tag names what the image is made from. Prints the
# fingerprint in that tag (packaging/cloud-image/inputs.sh), or explains on
# stderr and exits 1.
#
# The registry is pushed a tag once (packaging/cloud-image/publish.sh), so a
# changed entrypoint or build that kept its tag would pass CI and never be
# published. This checks that cannot happen: each file an image is made from
# moves the fingerprint, where the files sit does not, and the files that
# decide nothing the image contains leave it alone.
#
# Run by CI on every push and PR. Pure shell.

set -euo pipefail

cd "$(dirname "$0")/../packaging/cloud-image"

inputs=(build.sh build-docker.sh entrypoint)
others=(test.sh publish.sh README.md)

fingerprint="$(./inputs.sh)"
if [[ ! "$fingerprint" =~ ^[0-9a-f]{12}$ ]]; then
  echo "error: packaging/cloud-image/inputs.sh printed '$fingerprint', not twelve hex digits" >&2
  exit 1
fi

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
# The fingerprint of a copy of this directory with one more line in `$1`, or
# of a plain copy without an argument.
of_copy() {
  rm -rf "$scratch/copy"
  mkdir "$scratch/copy"
  cp "${inputs[@]}" "${others[@]}" "$scratch/copy/"
  if [[ $# -gt 0 ]]; then echo "# changed" >> "$scratch/copy/$1"; fi
  ./inputs.sh "$scratch/copy"
}

if [[ "$(of_copy)" != "$fingerprint" ]]; then
  echo "error: the same cloud image inputs in another directory have another fingerprint" >&2
  exit 1
fi
for file in "${inputs[@]}"; do
  if [[ "$(of_copy "$file")" == "$fingerprint" ]]; then
    echo "error: a change to packaging/cloud-image/$file keeps the image's tag, so it would never be published" >&2
    exit 1
  fi
done
for file in "${others[@]}"; do
  if [[ "$(of_copy "$file")" != "$fingerprint" ]]; then
    echo "error: a change to packaging/cloud-image/$file moves the image's tag, though the image does not hold it" >&2
    exit 1
  fi
done

# The app runs the image these inputs make: its pin names this fingerprint,
# so a change to the inputs moves the pin in the same pull request, as a box
# image change moves `PinnedImage::VERSION` (scripts/check-version.sh).
pins=../../crates/fletch-box/src/image_store/mod.rs
pinned="$(sed -n 's/^ *pub const CLOUD_FINGERPRINT: &'"'"'static str = "\([^"]*\)";.*/\1/p' "$pins")"
if [[ -z "$pinned" ]]; then
  echo "error: no PinnedImage::CLOUD_FINGERPRINT found in crates/fletch-box/src/image_store/mod.rs" >&2
  exit 1
fi
if [[ "$pinned" != "$fingerprint" ]]; then
  echo "error: the app pins cloud image inputs $pinned, but packaging/cloud-image/inputs.sh gives $fingerprint" >&2
  echo "Set PinnedImage::CLOUD_FINGERPRINT to $fingerprint in the same change; see packaging/cloud-image/README.md." >&2
  exit 1
fi

# The port the host has a provider forward to the guest's sshd is the
# entrypoint's default.
guest=../../crates/fletch-box/src/guest.rs
host_port="$(sed -n 's/^pub const CLOUD_SSH_PORT: u16 = \([0-9]*\);.*/\1/p' "$guest")"
entry_port="$(sed -n 's/^port=\${FLETCH_BOX_SSH_PORT:-\([0-9]*\)}.*/\1/p' entrypoint)"
if [[ -z "$host_port" || "$host_port" != "$entry_port" ]]; then
  echo "error: guest::CLOUD_SSH_PORT is '$host_port', but the entrypoint's default sshd port is '$entry_port'" >&2
  exit 1
fi

echo "$fingerprint"
