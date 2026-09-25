#!/usr/bin/env bash
#
# `bun tauri …` lands here (package.json's `tauri` script). Two things happen
# before the Tauri CLI runs:
#
#   1. The repo-root `.env` is exported, so OAuth keys and the like reach
#      `src-tauri/build.rs` in local dev (CI supplies them as real env).
#   2. For `dev` and `build` — the two subcommands that compile the Rust tree —
#      `scripts/doctor.sh` checks the toolchain first, so a missing cmake or a
#      stale SDK is one clear screen instead of a panic deep inside a
#      dependency's build script. Other subcommands (`signer`, `icon`, …)
#      don't compile anything and skip it. So does CI (`CI` is set on every
#      hosted runner): the release workflow provisions its own toolchain, and
#      a developer-machine preflight has no business gating a release.
#      `FLETCH_SKIP_DOCTOR=1` skips it anywhere.

set -uo pipefail
cd "$(dirname "$0")/.."

if [[ -f .env ]]; then
  set -a
  # shellcheck disable=SC1091
  . ./.env
  set +a
fi

if [[ -z "${CI:-}" ]]; then
  case "${1:-}" in
    dev|build)
      scripts/doctor.sh --quiet || exit 1
      ;;
  esac
fi

exec tauri "$@"
