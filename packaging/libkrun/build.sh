#!/bin/sh
# Build the VM runtime the box engine ships: libkrun with our patches, and
# libkrunfw (the guest kernel). macOS on Apple silicon, with Xcode, rustup and
# cargo; or Linux on x86-64 or arm64, with rustup, cargo and a C compiler that
# links static glibc binaries (libc6-dev). Vendored from Calina
# (sandbox/libkrun/build.sh) with Fletch paths.
#
#   build.sh [--unpatched]
#
# Writes to dist/ (next to this script):
#   libkrun.1.dylib     libkrun, patches/*.patch applied, no GPU, with disks (blk)
#   libkrunfw.5.dylib   Linux kernel bundle
#   manifest.txt        versions, patches and sha256s
# On Linux the same as libkrun.so.1 and libkrunfw.so.5 (their sonames; the
# kernel bundle is upstream's prebuilt libkrunfw-<arch>.tgz).
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
case "$(uname -sm)" in
  "Darwin arm64")
    OS=macos
    LIBKRUNFW_URL="https://github.com/libkrun/libkrunfw/releases/download/v$LIBKRUNFW_VERSION/libkrunfw-prebuilt-aarch64.tgz"
    LIBKRUNFW_SHA256=adef6739cc3bda2a1b57f04c08ccbfdf206a6c831d76b32de1beddc0e7984e86
    LIBKRUNFW_TGZ="libkrunfw-$LIBKRUNFW_VERSION-prebuilt-aarch64.tgz"
    LIBKRUN_FILE=libkrun.1.dylib LIBKRUNFW_FILE=libkrunfw.5.dylib
    ;;
  "Linux x86_64" | "Linux aarch64")
    OS=linux
    ARCH=$(uname -m)
    # Upstream's Linux build of the same kernel: libkrunfw.so.<version> with
    # the soname libkrunfw.so.5, which libkrun dlopens.
    LIBKRUNFW_URL="https://github.com/libkrun/libkrunfw/releases/download/v$LIBKRUNFW_VERSION/libkrunfw-$ARCH.tgz"
    case $ARCH in
      x86_64) LIBKRUNFW_SHA256=016c32ddb2a28aa300382cab33352d63a41a901eb36ee8d1ca24b389791c8b91 ;;
      aarch64) LIBKRUNFW_SHA256=dea7905a167eee17d482200ea2fe15871aceb293b0674ac825ed7ae69759399f ;;
    esac
    LIBKRUNFW_TGZ="libkrunfw-$LIBKRUNFW_VERSION-$ARCH.tgz"
    LIBKRUN_FILE=libkrun.so.1 LIBKRUNFW_FILE=libkrunfw.so.5
    ;;
  *) echo "build.sh: needs macOS on arm64, or Linux on x86_64 or aarch64" >&2; exit 1 ;;
esac

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
fetch "$LIBKRUNFW_URL" "$LIBKRUNFW_SHA256" "$BUILD/downloads/$LIBKRUNFW_TGZ"

echo "== libkrunfw $LIBKRUNFW_VERSION"
rm -rf "$BUILD/libkrunfw"
if [ "$OS" = macos ]; then
  # The prebuilt bundle is the kernel as a C array; compiling it gives the dylib.
  tar xzf "$BUILD/downloads/$LIBKRUNFW_TGZ" -C "$BUILD"
  make -s -C "$BUILD/libkrunfw" libkrunfw.5.dylib
  cp "$BUILD/libkrunfw/libkrunfw.5.dylib" "$DIST/"
  install_name_tool -id @rpath/libkrunfw.5.dylib "$DIST/libkrunfw.5.dylib"
else
  mkdir -p "$BUILD/libkrunfw"
  tar xzf "$BUILD/downloads/$LIBKRUNFW_TGZ" -C "$BUILD/libkrunfw"
  cp "$BUILD/libkrunfw/lib/$ARCH-linux-gnu/libkrunfw.so.$LIBKRUNFW_VERSION" "$DIST/$LIBKRUNFW_FILE"
fi

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

if [ "$OS" = macos ]; then
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
fi
# On Linux the init builds natively: the build script runs `cc -static`.
export CARGO_TARGET_DIR="$BUILD/target"
(cd "$SRC" && cargo build --release --locked --features blk)
if [ "$OS" = macos ]; then
  cp "$CARGO_TARGET_DIR/release/libkrun.dylib" "$DIST/libkrun.1.dylib"
  install_name_tool -id @rpath/libkrun.1.dylib "$DIST/libkrun.1.dylib"

  echo "== signing"
  # Ad hoc; the app bundle re-signs everything with its Developer ID.
  codesign -f -s - "$DIST/libkrun.1.dylib" "$DIST/libkrunfw.5.dylib"
else
  # Its soname is already libkrun.so.1 (libkrun's build.rs).
  cp "$CARGO_TARGET_DIR/release/libkrun.so" "$DIST/$LIBKRUN_FILE"
  strip --strip-unneeded "$DIST/$LIBKRUN_FILE"
fi

echo "== writing manifest.txt"
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
{
  echo "libkrun $LIBKRUN_VERSION $LIBKRUN_SHA256 (source tarball)"
  if [ "$OS" = macos ]; then
    echo "libkrunfw $LIBKRUNFW_VERSION $LIBKRUNFW_SHA256 (prebuilt kernel bundle)"
  else
    echo "libkrunfw $LIBKRUNFW_VERSION $LIBKRUNFW_SHA256 (upstream $ARCH build)"
  fi
  echo "features: blk (no gpu)"
  for p in $PATCHES; do echo "patch $(basename "$p") $(sha "$p")"; done
  for f in "$LIBKRUN_FILE" "$LIBKRUNFW_FILE"; do echo "dist $f $(sha "$DIST/$f")"; done
} > "$DIST/manifest.txt"

du -h "$DIST/$LIBKRUN_FILE" "$DIST/$LIBKRUNFW_FILE" | sed 's/^/   /'
echo "== done: $DIST"
