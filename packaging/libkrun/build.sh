#!/bin/sh
# Build the VM runtime the box engine ships: libkrun with our patches, and
# libkrunfw (the guest kernel). macOS on Apple silicon, with Xcode, rustup and
# cargo. Vendored from Calina (sandbox/libkrun/build.sh) with Fletch paths.
#
#   build.sh [--unpatched]
#
# Writes to dist/ (next to this script):
#   libkrun.1.dylib     libkrun, patches/*.patch applied, no GPU, with disks (blk)
#   libkrunfw.5.dylib   Linux kernel bundle
#   manifest.txt        versions, patches and sha256s
#
# --unpatched builds the same without the patches into dist-unpatched/, to see
# the patch checks fail. Downloads and the build tree live in build/. Respects
# an existing CARGO_HOME.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
BUILD="$HERE/build"
DIST="$HERE/dist"
PATCHES=$(find "$HERE/patches" -name '*.patch' | sort)
if [ "${1:-}" = --unpatched ]; then
  DIST="$HERE/dist-unpatched" PATCHES=
fi

LIBKRUN_VERSION=1.19.6
LIBKRUN_URL="https://github.com/libkrun/libkrun/archive/refs/tags/v$LIBKRUN_VERSION.tar.gz"
LIBKRUN_SHA256=7025d72208172dc06f791ad5af7ccaafeabdac9869f74ccb45fb6d4f6e5991cd
LIBKRUNFW_VERSION=5.6.2
LIBKRUNFW_URL="https://github.com/libkrun/libkrunfw/releases/download/v$LIBKRUNFW_VERSION/libkrunfw-prebuilt-aarch64.tgz"
LIBKRUNFW_SHA256=adef6739cc3bda2a1b57f04c08ccbfdf206a6c831d76b32de1beddc0e7984e86

[ "$(uname -sm)" = "Darwin arm64" ] || { echo "build.sh: needs macOS on arm64" >&2; exit 1; }
mkdir -p "$BUILD/downloads" "$DIST"

fetch() { # url sha256 file
  if [ ! -f "$3" ]; then
    curl -fsSL -o "$3.part" "$1"
    mv "$3.part" "$3"
  fi
  echo "$2  $3" | shasum -a 256 -c --quiet - || { rm -f "$3"; echo "build.sh: bad sha256: $1" >&2; exit 1; }
}

echo "== fetching sources"
fetch "$LIBKRUN_URL" "$LIBKRUN_SHA256" "$BUILD/downloads/libkrun-$LIBKRUN_VERSION.tar.gz"
fetch "$LIBKRUNFW_URL" "$LIBKRUNFW_SHA256" "$BUILD/downloads/libkrunfw-$LIBKRUNFW_VERSION-prebuilt-aarch64.tgz"

echo "== libkrunfw $LIBKRUNFW_VERSION"
# The prebuilt bundle is the kernel as a C array; compiling it gives the dylib.
rm -rf "$BUILD/libkrunfw"
tar xzf "$BUILD/downloads/libkrunfw-$LIBKRUNFW_VERSION-prebuilt-aarch64.tgz" -C "$BUILD"
make -s -C "$BUILD/libkrunfw" libkrunfw.5.dylib
cp "$BUILD/libkrunfw/libkrunfw.5.dylib" "$DIST/"
install_name_tool -id @rpath/libkrunfw.5.dylib "$DIST/libkrunfw.5.dylib"

echo "== libkrun $LIBKRUN_VERSION"
# A fresh tree every time, so the patches always apply to pristine sources.
# -m: new mtimes, or cargo would take files a patch no longer touches for
# unchanged and reuse stale objects. Dependencies and the sysroot stay cached.
SRC="$BUILD/libkrun-$LIBKRUN_VERSION"
rm -rf "$SRC"
tar xzmf "$BUILD/downloads/libkrun-$LIBKRUN_VERSION.tar.gz" -C "$BUILD"
for p in $PATCHES; do
  echo "   applying $(basename "$p")"
  patch -s -p1 -d "$SRC" < "$p"
done

# libkrun's init is a static Linux binary, cross-compiled with clang against a
# Debian sysroot that libkrun's Makefile downloads. It links with ld.lld; when
# that isn't installed, rustup's rust-lld works under that name.
SYSROOT="$BUILD/linux-sysroot"
make -s -C "$SRC" ROOTFS_DIR="$SYSROOT" "$SYSROOT/.sysroot_ready"
if ! command -v ld.lld > /dev/null; then
  mkdir -p "$BUILD/bin"
  ln -sf "$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin/rust-lld" "$BUILD/bin/ld.lld"
  PATH="$BUILD/bin:$PATH"
fi
GCC_LIB="$SYSROOT/usr/lib/gcc/aarch64-linux-gnu/12"
export CC_LINUX="/usr/bin/clang -target aarch64-linux-gnu -fuse-ld=lld -Wl,-strip-debug --sysroot $SYSROOT -B$GCC_LIB -L$GCC_LIB -Wno-c23-extensions"
# bindgen (in a build script) needs libclang. cargo runs directly, not through
# make: /usr/bin/make is SIP-protected and drops DYLD_* variables.
LIBCLANG_PATH="$(xcode-select -p)/Toolchains/XcodeDefault.xctoolchain/usr/lib"
export LIBCLANG_PATH DYLD_FALLBACK_LIBRARY_PATH="$LIBCLANG_PATH"
export CARGO_TARGET_DIR="$BUILD/target"
(cd "$SRC" && cargo build --release --locked --features blk)
cp "$CARGO_TARGET_DIR/release/libkrun.dylib" "$DIST/libkrun.1.dylib"
install_name_tool -id @rpath/libkrun.1.dylib "$DIST/libkrun.1.dylib"

echo "== signing"
# Ad hoc; the app bundle re-signs everything with its Developer ID.
codesign -f -s - "$DIST/libkrun.1.dylib" "$DIST/libkrunfw.5.dylib"

echo "== writing manifest.txt"
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
{
  echo "libkrun $LIBKRUN_VERSION $LIBKRUN_SHA256 (source tarball)"
  echo "libkrunfw $LIBKRUNFW_VERSION $LIBKRUNFW_SHA256 (prebuilt kernel bundle)"
  echo "features: blk (no gpu)"
  for p in $PATCHES; do echo "patch $(basename "$p") $(sha "$p")"; done
  for f in libkrun.1.dylib libkrunfw.5.dylib; do echo "dist $f $(sha "$DIST/$f")"; done
} > "$DIST/manifest.txt"

du -h "$DIST"/*.dylib | sed 's/^/   /'
echo "== done: $DIST"
