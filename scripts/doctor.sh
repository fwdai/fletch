#!/usr/bin/env bash
#
# Preflight for building the desktop app on macOS. Answers "why does the
# build fail on this machine?" up front, in one screen, instead of forty
# minutes into `cargo` inside some dependency's build script.
#
# Every check here is a failure someone has actually hit on a fresh Mac:
#
#   - No Command Line Tools: `/usr/bin/git`, `cc` and friends are shims that
#     pop Apple's installer dialog and exit non-zero.
#   - A stale SDK next to a freshly installed CLT: clang picks the newest
#     `MacOSX*.sdk` it can see, and if that one is a leftover from a beta
#     (say `MacOSX27.0.sdk` beside CLT 26.6), its `.tbd` stubs name an
#     architecture the older linker doesn't know ("tapi error: malformed
#     file … unknown architecture"). Every Rust crate then fails to link,
#     starting with the first build script.
#   - No cmake: `whisper-rs-sys` compiles whisper.cpp with it and panics with
#     "is `cmake` not installed?" (see docs/dictation.md).
#   - No Rust toolchain, or no Bun.
#
# Usage:
#   scripts/doctor.sh           report every check
#   scripts/doctor.sh --quiet   print only failures (what `bun tauri dev` runs)
#   scripts/doctor.sh --fix     also `brew install` what Homebrew can supply
#
# Exit 1 when anything fails. `FLETCH_SKIP_DOCTOR=1` bypasses it entirely.
# Not macOS? Nothing here applies; exits 0 saying so.

set -uo pipefail

quiet=0
fix=0
for arg in "$@"; do
  case "$arg" in
    --quiet) quiet=1 ;;
    --fix) fix=1 ;;
    -h|--help) sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "doctor: unknown option $arg" >&2; exit 2 ;;
  esac
done

if [[ "${FLETCH_SKIP_DOCTOR:-}" == "1" ]]; then
  exit 0
fi
if [[ "$(uname -s)" != "Darwin" ]]; then
  [[ $quiet -eq 1 ]] || echo "doctor: not macOS; nothing to check here."
  exit 0
fi

failures=0
ok()   { [[ $quiet -eq 1 ]] || printf '  ok    %s\n' "$1"; }
fail() {
  failures=$((failures + 1))
  printf '  FAIL  %s\n' "$1"
  shift
  for line in "$@"; do printf '        %s\n' "$line"; done
}

# --- Command Line Tools / Xcode -------------------------------------------

developer_dir="$(/usr/bin/xcode-select -p 2>/dev/null || true)"
if [[ -z "$developer_dir" || ! -d "$developer_dir" ]]; then
  fail "Xcode Command Line Tools are not installed" \
    "Fix:  xcode-select --install" \
    "(then re-run this script — the SDK and linker checks need them)"
  clt_ok=0
else
  ok "Command Line Tools at $developer_dir"
  clt_ok=1
fi

# --- SDK + linker agree ----------------------------------------------------
#
# Link a trivial C program against whatever SDK clang selects by default
# (`SDKROOT` when set, else the newest one under the selected developer dir).
# The probe is the actual failure mode, so it can't false-negative on a
# toolchain that merely looks odd.

if [[ $clt_ok -eq 1 ]]; then
  probe_dir="$(mktemp -d "${TMPDIR:-/tmp}/fletch-doctor.XXXXXX")"
  trap 'rm -rf "$probe_dir"' EXIT
  printf 'int main(void){return 0;}\n' > "$probe_dir/probe.c"

  links_with() {
    # $1: SDKROOT to use, or "" for clang's default.
    if [[ -n "$1" ]]; then
      SDKROOT="$1" cc "$probe_dir/probe.c" -o "$probe_dir/probe" 2> "$probe_dir/err"
    else
      cc "$probe_dir/probe.c" -o "$probe_dir/probe" 2> "$probe_dir/err"
    fi
  }

  selected_sdk="$(xcrun --sdk macosx --show-sdk-path 2>/dev/null || true)"
  if links_with "${SDKROOT:-}"; then
    ok "C toolchain links against ${selected_sdk:-the default SDK}"
  else
    # The linker's last lines, one array element each so `fail` indents every
    # line (macOS ships bash 3.2, so no `mapfile`).
    err_tail=()
    while IFS= read -r line; do err_tail+=("    $line"); done < <(tail -n 3 "$probe_dir/err")
    sdks_root="$(dirname "$selected_sdk")"
    # The SDK this CLT/Xcode actually shipped is what `MacOSX.sdk` points at.
    shipped="$(readlink "$sdks_root/MacOSX.sdk" 2>/dev/null || true)"
    if [[ -n "$shipped" && "$sdks_root/$shipped" != "$selected_sdk" ]] \
       && links_with "$sdks_root/$shipped"; then
      # A stale SDK is shadowing the shipped one. Name it, and every version
      # symlink that points at it, so the removal is one copy-paste.
      stale=("$selected_sdk")
      for link in "$sdks_root"/MacOSX*.sdk; do
        [[ -L "$link" && "$(readlink "$link")" == "$(basename "$selected_sdk")" ]] && stale+=("$link")
      done
      fail "The linker rejects the selected SDK ($(basename "$selected_sdk")); it is newer than the installed tools" \
        "$(basename "$selected_sdk") is a leftover (a beta CLT, most likely) beside the SDK these tools shipped with ($shipped)." \
        "clang picks the newest SDK it sees, and this linker can't read that one's library stubs:" \
        ${err_tail[@]+"${err_tail[@]}"} \
        "Fix (either):" \
        "  sudo rm -rf ${stale[*]}" \
        "  export SDKROOT=$sdks_root/$shipped   # in your shell profile, if you'd rather keep it"
    else
      fail "The C toolchain can't link a trivial program" \
        ${err_tail[@]+"${err_tail[@]}"} \
        "Fix:  reinstall the Command Line Tools —" \
        "      sudo rm -rf /Library/Developer/CommandLineTools && xcode-select --install"
    fi
  fi

  # The Swift dictation bridge (src-tauri/build.rs) needs the macOS 26 SDK.
  sdk_version="$(xcrun --sdk macosx --show-sdk-version 2>/dev/null || echo 0)"
  if [[ "${sdk_version%%.*}" -ge 26 ]]; then
    ok "macOS SDK $sdk_version (the dictation bridge needs 26+)"
  else
    fail "macOS SDK $sdk_version is too old; the Swift dictation bridge needs the macOS 26 SDK" \
      "Fix:  install Xcode 26 / Command Line Tools 26 or newer, then" \
      "      sudo xcode-select -s /Applications/Xcode.app   # if you installed Xcode"
  fi
fi

# --- cmake (whisper.cpp) -----------------------------------------------------

if command -v cmake > /dev/null 2>&1; then
  ok "cmake $(cmake --version | head -n 1 | awk '{print $3}')"
else
  if [[ $fix -eq 1 ]] && command -v brew > /dev/null 2>&1; then
    echo "doctor: installing cmake with Homebrew…"
    if brew install cmake && command -v cmake > /dev/null 2>&1; then
      ok "cmake $(cmake --version | head -n 1 | awk '{print $3}') (installed)"
    else
      fail "cmake install failed" "Fix:  brew install cmake"
    fi
  elif command -v brew > /dev/null 2>&1; then
    fail "cmake is not installed (whisper-rs-sys builds whisper.cpp with it)" \
      "Fix:  brew install cmake        (or: scripts/doctor.sh --fix)"
  else
    fail "cmake is not installed (whisper-rs-sys builds whisper.cpp with it)" \
      "Fix:  install Homebrew (https://brew.sh), then: brew install cmake" \
      "      or the official installer: https://cmake.org/download/ — and put cmake on PATH"
  fi
fi

# --- Rust -------------------------------------------------------------------

if command -v cargo > /dev/null 2>&1; then
  # rust-toolchain.toml pins the version; rustup installs it on first use.
  ok "$(cargo --version 2>/dev/null || echo cargo)"
else
  fail "Rust is not installed" \
    "Fix:  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" \
    "      then open a new shell (rustup picks the pinned toolchain from rust-toolchain.toml)"
fi

# --- Bun --------------------------------------------------------------------

if command -v bun > /dev/null 2>&1; then
  ok "bun $(bun --version)"
else
  fail "Bun is not installed" \
    "Fix:  curl -fsSL https://bun.sh/install | bash"
fi

# ---------------------------------------------------------------------------

if [[ $failures -gt 0 ]]; then
  echo
  echo "doctor: $failures problem(s). Fix the above, then re-run: bun run doctor"
  exit 1
fi
[[ $quiet -eq 1 ]] || echo "doctor: all good."
exit 0
