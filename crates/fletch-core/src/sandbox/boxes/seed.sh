#!/bin/bash
# Reduces the workspace disk this bench booted on to a project's seed: what a
# new workspace of the project may start with. Run as the guest user.
#
#   seed.sh <the project's toolchain cache on this disk, or ""> \
#           <KiB a directory must hold to be kept> \
#           <the name of a directory never kept: Fletch's own index> \
#           [<path in a checkout never kept>...]
#
# Kept: in each checkout under /data/ws, the directories git ignores as a
# whole and that hold at least the given KiB (node_modules, target, .venv,
# build output), where they were; and the project's toolchain cache (package
# downloads, Playwright's browsers).
# Removed: everything else on the disk the guest user owns. The checkout's
# tracked and untracked files and its .git, so no work, branch or commit of
# the workspace the disk came from is left. Ignored files that are not a whole
# directory (.env, a local database) and ignored directories under the given
# size (certificates, local settings), since those are a workspace's own. The
# paths the project excludes, whatever is there. The rest of the home, the
# mailbox and the launch env files.
#
# Prints the KiB kept on its last line. Any failure exits non-zero, and the
# host then discards the disk.
set -euo pipefail

CACHE=$1
WORTH_INHERITING_KIB=$2
NOT_INHERITED=$3
shift 3

WS=/data/ws
KEPT=$WS/.fletch-seed-kept
KEPT_CACHE=$WS/.fletch-seed-cache
rm -rf "$KEPT" "$KEPT_CACHE"
mkdir "$KEPT"
for checkout in "$WS"/*/; do
  checkout=${checkout%/}
  [ "$(git -C "$checkout" rev-parse --show-toplevel 2>/dev/null)" = "$checkout" ] || continue
  name=${checkout##*/}
  git -C "$checkout" ls-files -z --others --ignored --exclude-standard --directory |
    while IFS= read -r -d '' ignored; do
      case "$ignored" in */) ;; *) continue ;; esac
      dir=${ignored%/}
      [ "${dir##*/}" != "$NOT_INHERITED" ] || continue
      [ "$(du -sk "$checkout/$dir" | cut -f1)" -ge "$WORTH_INHERITING_KIB" ] || continue
      mkdir -p "$KEPT/$name/$(dirname "$dir")"
      mv "$checkout/$dir" "$KEPT/$name/$dir"
    done
  for excluded; do
    rm -rf "${KEPT:?}/$name/$excluded"
  done
done

[ ! -d "$CACHE" ] || mv "$CACHE" "$KEPT_CACHE"

find "$WS" -mindepth 1 -maxdepth 1 ! -path "$KEPT" ! -path "$KEPT_CACHE" -exec rm -rf {} +
find "$KEPT" -mindepth 1 -maxdepth 1 -exec mv {} "$WS"/ \;
rmdir "$KEPT"
for dir in /data/home /data/rpc /data/run /data/tools; do
  find "$dir" -mindepth 1 -delete
done
if [ -d "$KEPT_CACHE" ]; then
  mkdir -p "$(dirname "$CACHE")"
  mv "$KEPT_CACHE" "$CACHE"
fi
sync
du -sk "$WS" /data/home | awk '{ kib += $1 } END { print kib }'
