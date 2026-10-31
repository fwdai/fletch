#!/bin/bash
# Spike: mount the images build-layers.sh wrote the way the guest init would
# and run the checks README.md lists. Root, privileged container, network.
#
#   test-layers.sh OUT_DIR
#
# Writes OUT_DIR/report.md (one row per check) and exits non-zero if any failed.
# WITH_AGENTS=0 skips the agent CLI installs.
set -u

OUT=${1:?usage: test-layers.sh OUT_DIR}
OUT=$(cd "$OUT" && pwd)
HERE=$(cd "$(dirname "$0")" && pwd)
WITH_AGENTS=${WITH_AGENTS:-1}
MNT=/mnt/spike
M=$MNT/merged
REPORT=$OUT/report.md
LOG=$OUT/test.log
: > "$REPORT"
: > "$LOG"
failed=0

say() { echo "== $*" | tee -a "$LOG"; }
row() { printf '| %s | %s | %s |\n' "$1" "$2" "$(echo "$3" | tr '|\n' '/ ' | cut -c1-160)" >> "$REPORT"; }
pass() { say "PASS $1: $2"; row "$1" PASS "$2"; }
fail() { say "FAIL $1: $2"; row "$1" "**FAIL**" "$2"; failed=1; }
# check NAME DESCRIPTION -- command...: PASS when the command exits 0; its last
# output line is the note.
check() {
  local name=$1 desc=$2; shift 2; [ "$1" = -- ] && shift
  local out
  if out=$(timeout 900 "$@" 2>&1); then
    pass "$name" "$desc: $(echo "$out" | tail -1)"
  else
    echo "$out" >> "$LOG"
    fail "$name" "$desc: $(echo "$out" | tail -3 | tr '\n' ' ')"
  fi
}

modprobe erofs 2>/dev/null || true
modprobe overlay 2>/dev/null || true
echo "## Checks" > "$REPORT"
echo >> "$REPORT"
echo "| Check | Result | Note |" >> "$REPORT"
echo "| --- | --- | --- |" >> "$REPORT"

# ------------------------------------------------------------ mounting
mkdir -p "$MNT/base" "$MNT/devtools" "$MNT/browser" "$M"
mount -t erofs -o loop,ro "$OUT/base.erofs" "$MNT/base" || { fail T-mount "base.erofs does not mount: $(dmesg | tail -2)"; exit 1; }
mount -t erofs -o loop,ro "$OUT/layer-devtools.erofs" "$MNT/devtools" || fail T-mount "layer-devtools.erofs does not mount"
mount -t erofs -o loop,ro "$OUT/layer-browser.erofs" "$MNT/browser" || fail T-mount "layer-browser.erofs does not mount"
pass T-mount "all three images mount ($(mount | grep -c "$MNT/.* type erofs") erofs mounts); kernel $(uname -r)"

teardown() {
  umount -R "$M" 2>/dev/null || true
}
# assemble LAYER...: the merged root, exactly as the guest init builds it:
# the base bound at $M, then an overlay per top-level directory the layers
# carry, first argument on top.
assemble() {
  teardown
  mount --bind "$MNT/base" "$M"
  mount -o remount,bind,ro "$M"
  local d lower l
  for d in usr etc var; do
    lower=""
    for l in "$@"; do
      [ -d "$MNT/$l/$d" ] && lower="$lower$MNT/$l/$d:"
    done
    [ -n "$lower" ] || continue
    mount -t overlay overlay -o "ro,lowerdir=${lower}$MNT/base/$d" "$M/$d" \
      || fail T-overlay "overlay on /$d with $*: $(dmesg | tail -1)"
  done
  mount -t proc proc "$M/proc"
  mount --rbind /dev "$M/dev"
  mount --make-rslave "$M/dev"
  mount -t tmpfs -o mode=1777 tmpfs "$M/tmp"
  mount -t tmpfs -o mode=755 tmpfs "$M/run"
  mount -t tmpfs -o mode=1777 tmpfs "$M/var/tmp"
  mount -t tmpfs -o mode=755 tmpfs "$M/data"
  mkdir -p "$M/data/home" "$M/data/ws"
  chown 1000:1000 "$M/data/home" "$M/data/ws"
  # The container's resolver, so npm and friends reach the network.
  mkdir -p "$M/run/resolv"
  cp /etc/resolv.conf "$M/run/resolv/resolv.conf"
  mount --bind "$M/run/resolv/resolv.conf" "$M/etc/resolv.conf"
}
ENVI=(/usr/bin/env -i HOME=/data/home USER=agent LOGNAME=agent PATH=/usr/local/bin:/usr/bin:/bin LANG=C.UTF-8 TERM=xterm SHELL=/bin/bash)
run() { chroot --userspec=1000:1000 --groups=1000 "$M" "${ENVI[@]}" "$@"; }
runroot() { chroot "$M" /usr/bin/env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin LANG=C.UTF-8 "$@"; }
sh_as_agent() { run bash -lc "$1"; }

# ------------------------------------------------------------ default: base + devtools
say "configuration: base + devtools"
assemble devtools
check T-base-tools "bash, git, rg, curl, jq, node, npm present" -- \
  sh_as_agent 'bash --version | head -1 && git --version && rg --version | head -1 && curl --version | head -1 && jq --version && node -v && npm -v'
check T-base-noperl-ok "git subcommands that need perl are not on the critical path (git init/add/commit work)" -- \
  sh_as_agent 'cd $(mktemp -d) && git init -q . && git -c user.email=a@b -c user.name=a commit -q --allow-empty -m x && git log --oneline | head -1'
check T-sshd "sshd -t with the box config and a generated host key; sshd user exists; no systemd package" -- \
  runroot sh -c 'getent passwd sshd >/dev/null && ssh-keygen -q -t ed25519 -N "" -f /tmp/hk && /usr/sbin/sshd -t -f /etc/ssh/sshd_config.fletch -h /tmp/hk && ! test -x /usr/lib/systemd/systemd && echo "sshd config ok, systemd absent"'
check T-nollvm-files "Mesa/LLVM files are excluded from the tree" -- \
  sh -c "! ls $M/usr/lib/aarch64-linux-gnu/libLLVM* $M/usr/lib/aarch64-linux-gnu/libgallium* $M/usr/lib/aarch64-linux-gnu/libz3* 2>/dev/null && ls $M/usr/lib/aarch64-linux-gnu/libgbm.so.1 && echo 'libgbm present, backend absent'"
check T-locale "no /usr/share/locale, C.UTF-8 works" -- \
  sh_as_agent 'test ! -d /usr/share/locale/de && locale 2>&1 | head -1 && python3 -c "import sys; print(sys.getfilesystemencoding())"'
check T-fonts "fontconfig resolves sans-serif, emoji and CJK on the base font set" -- \
  sh_as_agent 'fc-match sans-serif && fc-match emoji && fc-match "sans-serif:lang=zh" && fc-list | wc -l'
check T-dev-gcc "devtools: gcc compiles and runs a C program" -- \
  sh_as_agent 'cd $(mktemp -d) && printf "#include <stdio.h>\nint main(){puts(\"hello from gcc\");return 0;}\n" > h.c && gcc -O2 h.c -o h && ./h && g++ --version | head -1'
check T-dev-python "devtools: python3 with ssl, sqlite3, pip and venv" -- \
  sh_as_agent 'python3 -c "import ssl, sqlite3, json; print(\"py ok\")" && pip --version && python3 -m venv /tmp/venv && /tmp/venv/bin/python -c "print(\"venv ok\")"'
check T-ldd "no missing shared libraries for python3, gcc, node, git, sshd through the overlay" -- \
  runroot sh -c 'for b in /usr/bin/python3 /usr/bin/gcc /usr/bin/git /usr/sbin/sshd /usr/local/bin/node /usr/bin/rg; do ldd $b | grep -q "not found" && { echo "$b: missing libs"; ldd $b | grep "not found"; exit 1; }; done; echo "all resolve"'
check T-dev-nodegyp "devtools: node-gyp builds a native addon against the Node headers in the layer" -- \
  sh_as_agent 'set -e; cd $(mktemp -d); test -f /usr/local/include/node/node_api.h; npm init -y >/dev/null; npm i --silent node-gyp@latest node-addon-api@latest; cat > binding.gyp <<EOF
{ "targets": [ { "target_name": "hello", "sources": [ "hello.cc" ], "include_dirs": ["<!@(node -p \"require(\\"node-addon-api\\").include\")"], "defines": ["NAPI_DISABLE_CPP_EXCEPTIONS"] } ] }
EOF
printf "#include <napi.h>\nNapi::String Hi(const Napi::CallbackInfo& i){return Napi::String::New(i.Env(),\"hi from addon\");}\nNapi::Object Init(Napi::Env e, Napi::Object x){x.Set(\"hi\",Napi::Function::New(e,Hi));return x;}\nNODE_API_MODULE(hello, Init)\n" > hello.cc
npx node-gyp rebuild --nodedir=/usr/local >/dev/null 2>&1 || npx node-gyp rebuild --nodedir=/usr/local
node -e "console.log(require(\"./build/Release/hello.node\").hi())"'
check T-no-browser-layer "chromium is absent without the browser layer" -- \
  sh -c "! test -e $M/usr/bin/chromium && echo absent"
check T-playwright "Playwright downloads chrome-headless-shell and screenshots on the base libraries" -- \
  sh_as_agent 'set -e; cd $(mktemp -d); npm init -y >/dev/null; npm i --silent playwright; npx playwright install chromium-headless-shell 2>&1 | tail -2; cat > s.mjs <<EOF
import { chromium } from "playwright";
const b = await chromium.launch({ args: ["--no-sandbox", "--disable-dev-shm-usage"] });
const p = await b.newPage();
await p.setContent("<h1 style=font:40px/1.2\ sans-serif>Hello 你好 Привет 😀</h1><canvas id=c></canvas><script>c.getContext(\"2d\").fillRect(0,0,50,50)</script>");
await p.screenshot({ path: "shot.png" });
await b.close();
EOF
node s.mjs && stat -c "%s bytes" shot.png && test $(stat -c %s shot.png) -gt 2000'
check T-fontcache-pw "no fontconfig cache was rebuilt into HOME by the Playwright browser" -- \
  sh_as_agent 'n=$(ls ~/.cache/fontconfig 2>/dev/null | wc -l); echo "$n cache files in HOME"; test "$n" -eq 0'

# overlay cost: a cold recursive scan of /usr, three-layer merge vs the plain base
drop_caches() { sync; echo 3 > /proc/sys/vm/drop_caches 2>/dev/null || true; }
assemble browser devtools
drop_caches
t0=$(date +%s%N); find "$M/usr" -printf '' ; t1=$(date +%s%N)
merged_ms=$(( (t1 - t0) / 1000000 ))
drop_caches
t0=$(date +%s%N); find "$MNT/base/usr" -printf '' ; t1=$(date +%s%N)
base_ms=$(( (t1 - t0) / 1000000 ))
if [ "$base_ms" -gt 0 ] && [ $(( merged_ms * 10 )) -le $(( base_ms * 30 )) ]; then
  pass T-overlay-perf "cold find over /usr: base ${base_ms} ms, 3-layer overlay ${merged_ms} ms ($(awk "BEGIN{printf \"%.1f\", $merged_ms/($base_ms?$base_ms:1)}")x)"
else
  fail T-overlay-perf "cold find over /usr: base ${base_ms} ms, overlay ${merged_ms} ms (more than 3x)"
fi

# ------------------------------------------------------------ base + devtools + browser
say "configuration: base + devtools + browser"
check T-chromium "browser layer: headless Chromium screenshots text, emoji and a canvas; no Mesa/LLVM loaded" -- \
  sh_as_agent 'set -e; cd $(mktemp -d); printf "<body style=\"font:32px sans-serif\">Hello 你好 Привет 😀 <canvas id=c width=80 height=40></canvas><script>c.getContext(\"2d\").fillStyle=\"red\";c.getContext(\"2d\").fillRect(0,0,80,40)</script></body>" > p.html
LD_DEBUG=libs chromium --headless=new --no-sandbox --disable-dev-shm-usage --window-size=600,200 --screenshot=s.png file://$PWD/p.html 2>ld.txt >/dev/null
test $(stat -c %s s.png) -gt 2000
! grep -E "calling init: .*(gallium|LLVM|libz3|libGL|gtk)" ld.txt
echo "$(stat -c %s s.png) byte png; $(grep -c "calling init" ld.txt) libs loaded; chromium $(chromium --version)"'
check T-fontcache "no fontconfig cache rebuilt into HOME by the system Chromium" -- \
  sh_as_agent 'n=$(ls ~/.cache/fontconfig 2>/dev/null | wc -l); echo "$n cache files in HOME"; test "$n" -eq 0'
check T-fonts-noto "browser layer adds fonts-noto-core (Devanagari resolves to Noto)" -- \
  sh_as_agent 'fc-match "sans-serif:lang=hi" | grep -i noto'
check T-ldd-chromium "chromium resolves every library through the overlay" -- \
  runroot sh -c 'ldd /usr/lib/chromium/chromium | grep "not found" && exit 1; echo "all resolve"'

# ------------------------------------------------------------ base only: the agent CLIs
say "configuration: base only"
assemble
check T-base-only-no-dev "gcc and python3 are absent on the base alone" -- \
  sh -c "! test -e $M/usr/bin/gcc && ! test -e $M/usr/bin/python3 && echo absent"
if [ "$WITH_AGENTS" = 1 ]; then
  check T-agents-claude "Claude Code installs and starts on the base" -- \
    sh_as_agent 'set -e; P=/tmp/t/claude; mkdir -p $P; npm install -g --prefix $P --cache /tmp/npm-cache --no-fund --no-audit --silent @anthropic-ai/claude-code@latest; $P/bin/claude --version'
  check T-agents-codex "Codex installs and starts on the base" -- \
    sh_as_agent 'set -e; P=/tmp/t/codex; mkdir -p $P; npm install -g --prefix $P --cache /tmp/npm-cache --no-fund --no-audit --silent @openai/codex@latest; $P/bin/codex --version'
  check T-agents-opencode "OpenCode installs and starts on the base" -- \
    sh_as_agent 'set -e; P=/tmp/t/opencode; mkdir -p $P; npm install -g --prefix $P --cache /tmp/npm-cache --no-fund --no-audit --silent opencode-ai@latest; $P/bin/opencode --version'
  check T-agents-pi "Pi installs and starts on the base (needs the system Node)" -- \
    sh_as_agent 'set -e; P=/tmp/t/pi; mkdir -p $P; npm install -g --prefix $P --cache /tmp/npm-cache --no-fund --no-audit --silent @earendil-works/pi-coding-agent@latest; $P/bin/pi --version'
  check T-agents-cursor "Cursor CLI installs (glibc, libstdc++, bash) and starts on the base" -- \
    sh_as_agent 'set -e; P=/tmp/t/cursor; mkdir -p $P; curl -fsSL https://cursor.com/install -o /tmp/cursor-install.sh; HOME=$P bash /tmp/cursor-install.sh >/dev/null 2>&1 || HOME=$P bash /tmp/cursor-install.sh; $P/.local/bin/cursor-agent --version'
else
  row T-agents SKIPPED "WITH_AGENTS=0"
fi
teardown

# ------------------------------------------------------------ images themselves
sizes=$(sed -n '/^| Image/,/^$/p' "$OUT/sizes.md")
base_mb=$(( $(stat -c %s "$OUT/base.erofs") / 1048576 ))
dev_mb=$(( $(stat -c %s "$OUT/layer-devtools.erofs") / 1048576 ))
br_mb=$(( $(stat -c %s "$OUT/layer-browser.erofs") / 1048576 ))
old_mb=$(( $(stat -c %s "$OUT/base-oldflags.erofs") / 1048576 ))
if [ "$base_mb" -le 200 ] && [ "$dev_mb" -le 150 ] && [ "$br_mb" -le 180 ]; then
  pass T-sizes "base $base_mb MB (today's flags: $old_mb MB), devtools $dev_mb MB, browser $br_mb MB; default download $((base_mb + dev_mb)) MB"
else
  fail T-sizes "base $base_mb MB (today's flags: $old_mb MB), devtools $dev_mb MB, browser $br_mb MB; targets 200/150/180"
fi
check T-repro "mkfs.erofs twice on the same tree gives identical images" -- sh -c "grep -q identical '$OUT/repro.txt' && cat '$OUT/repro.txt'"
delta=$(python3 "$HERE/chunk-delta.py" "$OUT/base.erofs" "$OUT/base-v2.erofs" 2>&1 | tail -1)
echo "$delta" >> "$LOG"
reused=$(echo "$delta" | sed -n 's/.*reused_pct=\([0-9.]*\).*/\1/p')
if [ -n "$reused" ] && awk "BEGIN{exit !($reused >= 80)}"; then
  pass T-delta "a rebuild with one new 1 MiB file and two edits: $delta"
else
  fail T-delta "chunk reuse below 80%: $delta"
fi
rm -f "$OUT/base-v2.erofs"

{
  echo
  cat "$OUT/sizes.md"
} >> "$REPORT"
say "report: $REPORT"
exit $failed
