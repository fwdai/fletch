#!/bin/sh
# Installs Cursor's CLI onto a tools disk; see claude.sh for the contract. The
# CLI ends up at /opt/fletch/tools/cursor/bin/cursor-agent.
#
#   cursor.sh <version>|latest
#
# Cursor's installer (https://cursor.com/install) has no version flag: it
# unpacks the current release under $HOME/.local/share/cursor-agent/versions/<v>
# and links $HOME/.local/bin/cursor-agent to it. Pointing HOME at the prefix
# keeps everything on the disk; asking for a version means checking that the
# one that landed is it, so a disk named for a version never holds another.
set -eu
VERSION=${1:?usage: cursor.sh <version>|latest}
PREFIX=/data/tools/cursor

mkdir -p "$PREFIX/bin"
curl -fsSL https://cursor.com/install -o /tmp/cursor-install.sh
HOME=$PREFIX bash /tmp/cursor-install.sh
# A relative link: the prefix is /data/tools/cursor now and /opt/fletch/tools/cursor later.
target=$(readlink -f "$PREFIX/.local/bin/cursor-agent")
ln -sfn "..${target#"$PREFIX"}" "$PREFIX/bin/cursor-agent"

installed=$("$PREFIX/bin/cursor-agent" --version)
echo "$installed"
[ "$VERSION" = latest ] || case "$installed" in
  *"$VERSION"*) ;;
  *) echo "cursor.sh: asked for $VERSION, the installer delivered $installed" >&2; exit 1;;
esac
