#!/bin/bash
# Test the images build.sh wrote: mount them the way the guest init does (the
# base bound at a root, each layer's directories overlaid on it) and check,
# from a chroot as the `agent` user, what the box engine and the agents rely on.
# Root, privileged container (loop mounts, overlayfs, chroot), network for the
# npm and browser downloads. The Box image workflow runs it on every build.
#
#   test.sh OUT_DIR
#
# Writes OUT_DIR/report.md (one row per check) and OUT_DIR/test.log, and exits
# non-zero when a check failed. WITH_AGENTS=0 skips the agent CLI installs
# (about 1 GB of downloads).
set -u

OUT=${1:?usage: test.sh OUT_DIR}
OUT=$(cd "$OUT" && pwd)
WITH_AGENTS=${WITH_AGENTS:-1}
MNT=/mnt/box-image-test
M=$MNT/merged
# The multiarch library directory: the images are this host's architecture.
LIB=$M/usr/lib/$(uname -m)-linux-gnu
REPORT=$OUT/report.md
LOG=$OUT/test.log
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
  if out=$("$@" 2>&1); then
    pass "$name" "$desc: $(echo "$out" | tail -1)"
  else
    echo "$out" >> "$LOG"
    fail "$name" "$desc: $(echo "$out" | tail -3 | tr '\n' ' ')"
  fi
}

modprobe erofs 2>/dev/null || true
modprobe overlay 2>/dev/null || true
{
  echo "## Checks"
  echo
  echo "| Check | Result | Note |"
  echo "| --- | --- | --- |"
} > "$REPORT"

# ------------------------------------------------------------ mounting
layers=$(jq -r '.images[] | select(.name != "base") | .name' "$OUT/manifest.json")
mkdir -p "$MNT/base" "$M"
mount -t erofs -o loop,ro "$OUT/base.erofs" "$MNT/base" || { fail T-mount "base.erofs does not mount: $(dmesg | tail -2)"; exit 1; }
for l in $layers; do
  mkdir -p "$MNT/$l"
  mount -t erofs -o loop,ro "$OUT/layer-$l.erofs" "$MNT/$l" || fail T-mount "layer-$l.erofs does not mount"
  [ -f "$MNT/$l/.fletch-layer" ] || fail T-mount "layer-$l.erofs has no /.fletch-layer marker"
done
pass T-mount "every image mounts and each layer carries its marker ($(mount | grep -c "$MNT/.* type erofs") erofs mounts; kernel $(uname -r))"

teardown() { umount -R "$M" 2>/dev/null || true; umount "$MNT/sys" 2>/dev/null || true; }
# assemble LAYER...: the merged root as the guest init builds it: the base
# bound at $M, and on /usr, /etc and /var an overlay of the layers' directories
# (first argument on top) on the base's, under an empty upper layer of the
# box's own changes (a tmpfs here, the data disk in a box).
assemble() {
  teardown
  mount --bind "$MNT/base" "$M"
  mount -o remount,bind,ro "$M"
  mkdir -p "$MNT/sys"
  mount -t tmpfs tmpfs "$MNT/sys"
  local d lower l
  for d in usr etc var; do
    lower=""
    for l in "$@"; do
      [ -d "$MNT/$l/$d" ] && lower="$lower$MNT/$l/$d:"
    done
    mkdir -p "$MNT/sys/$d/upper" "$MNT/sys/$d/work"
    mount -t overlay overlay \
      -o "lowerdir=${lower}$MNT/base/$d,upperdir=$MNT/sys/$d/upper,workdir=$MNT/sys/$d/work" "$M/$d" \
      || fail T-overlay "overlay on /$d with $*: $(dmesg | tail -1)"
  done
  mount -t proc proc "$M/proc"
  mount --rbind /dev "$M/dev"
  mount --make-rslave "$M/dev"
  mount -t tmpfs -o mode=1777 tmpfs "$M/tmp"
  mount -t tmpfs -o mode=755 tmpfs "$M/run"
  mkdir -m 755 "$M/run/sshd"
  mount -t tmpfs -o mode=1777 tmpfs "$M/var/tmp"
  mount -t tmpfs -o mode=755 tmpfs "$M/data"
  mkdir -p "$M/data/home" "$M/data/ws" "$M/data/ssh"
  chown 1000:1000 "$M/data/home" "$M/data/ws"
  # The container's resolver, so npm and friends reach the network.
  mkdir -p "$M/run/resolv"
  cp /etc/resolv.conf "$M/run/resolv/resolv.conf"
  mount --bind "$M/run/resolv/resolv.conf" "$M/etc/resolv.conf"
}
ENVI=(/usr/bin/env -i HOME=/data/home USER=agent LOGNAME=agent PATH=/usr/local/bin:/usr/bin:/bin LANG=C.UTF-8 TERM=xterm SHELL=/bin/bash)
run() { chroot --userspec=1000:1000 --groups=1000 "$M" "${ENVI[@]}" "$@"; }
runroot() { chroot "$M" /usr/bin/env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin LANG=C.UTF-8 "$@"; }
agent_sh() { run bash -c "$1"; }
# forgets BEFORE AFTER: forget-0.5.0-docker-proxies, run as init runs it, turns
# the agent's docker config BEFORE into AFTER and keeps its mode.
forgets() {
  local config=$M/data/home/.docker/config.json
  mkdir -p "${config%/*}"
  printf '%s' "$1" > "$config"
  chmod 600 "$config"
  chown -R 1000:1000 "${config%/*}"
  run /opt/fletch/forget-0.5.0-docker-proxies /data/home/.docker/config.json || return 1
  [ "$(jq -c . "$config")" = "$2" ] && [ "$(stat -c %a "$config")" = 600 ] && [ ! -e "$config.new" ] ||
    { echo "$1 became $(jq -c . "$config"), mode $(stat -c %a "$config")"; return 1; }
}
forgets_only_what_0_5_0_wrote() {
  local written='"httpProxy":"http://172.17.0.1:8899","httpsProxy":"http://172.17.0.1:8899","noProxy":"localhost,127.0.0.0/8,::1,172.16.0.0/12,192.168.0.0/16"'
  local other_daemon='"tcp://other:2375":{"httpProxy":"http://corp:3128"}'
  local own='{"proxies":{"default":{"httpProxy":"http://corp:3128"}}}'
  forgets "{\"auths\":{\"r\":{}},\"proxies\":{\"default\":{$written}}}" '{"auths":{"r":{}}}' &&
    forgets "{\"proxies\":{\"default\":{$written},$other_daemon}}" "{\"proxies\":{$other_daemon}}" &&
    forgets "{\"proxies\":{\"default\":{$written,\"ftpProxy\":\"http://corp:3128\"}}}" '{"proxies":{"default":{"ftpProxy":"http://corp:3128"}}}' &&
    forgets '{"proxies":{"default":{"httpProxy":"http://172.17.0.1:8899","httpsProxy":"http://corp:3128","noProxy":"internal"}}}' '{"proxies":{"default":{"httpsProxy":"http://corp:3128","noProxy":"internal"}}}' &&
    forgets "$own" "$own" &&
    run /opt/fletch/forget-0.5.0-docker-proxies /data/home/.docker/absent.json &&
    echo "its own values go, alone or beside an added daemon, an added key or changed keys; the agent's own proxy and a missing file are left"
}
PAGE='<body style="font:32px sans-serif">Hello 你好 Привет 😀 <canvas id=c width=80 height=40></canvas><script>c.getContext("2d").fillStyle="red";c.getContext("2d").fillRect(0,0,80,40)</script></body>'

# ------------------------------------------------------------ base + devtools (the default)
say "configuration: base + devtools"
assemble devtools
check T-base-tools "bash, git, rg, curl, jq, node, npm, corepack present" -- \
  agent_sh 'bash --version | head -1 && git --version && rg --version | head -1 && curl --version | head -1 && jq --version && node -v && npm -v && corepack --version'
# The Run panel's detected commands for a pnpm or yarn project: corepack's
# shims fetch the manager a package.json names and run it.
check T-base-pm "pnpm and yarn run through corepack's shims, the version a project pins" -- \
  agent_sh 'set -e; test -L /usr/local/bin/pnpm && test -L /usr/local/bin/yarn && test -L /usr/local/bin/pnpx && test -L /usr/local/bin/yarnpkg
cd $(mktemp -d); echo "{\"name\":\"t\",\"packageManager\":\"pnpm@10.0.0\",\"scripts\":{\"v\":\"pnpm --version\"}}" > package.json
v=$(pnpm --version); [ "$v" = 10.0.0 ] || { echo "pnpm ran $v, not the pinned 10.0.0"; exit 1; }
echo "{\"name\":\"t\",\"packageManager\":\"yarn@1.22.22\"}" > package.json
y=$(yarn --version); [ "$y" = 1.22.22 ] || { echo "yarn ran $y, not the pinned 1.22.22"; exit 1; }
echo "pnpm $v, yarn $y, as pinned"'
check T-git "git init, commit and log work as agent" -- \
  agent_sh 'cd $(mktemp -d) && git init -q . && git -c user.email=a@b -c user.name=a commit -q --allow-empty -m x && git log --oneline | head -1'
check T-sshd "sshd -t with the box config and a host key under /data/ssh; sshd user exists; no systemd" -- \
  runroot sh -c 'getent passwd sshd >/dev/null && ssh-keygen -q -t ed25519 -N "" -f /data/ssh/host_key && /usr/sbin/sshd -t -f /etc/ssh/sshd_config.fletch && ! test -x /usr/lib/systemd/systemd && echo "sshd config ok, systemd absent"'
check T-guest "fletch-guest is in the image and answers" -- \
  agent_sh 'test -x /opt/fletch/bin/fletch-guest && test -L /usr/local/bin/fletch-guest && cat /etc/fletch-image'
# How an agent launch gets its arguments: as a file, so that together they may
# exceed what one argument can be (Linux caps one at 128 KiB). The app's
# launcher relies on it from image 0.3.4.
check T-guest-args "fletch-guest exec hands the command the arguments in --args-file, then removes it" -- \
  agent_sh 'd=$(mktemp -d); printf "PATH=/usr/bin:/bin\n" > "$d/e"; for c in a b c; do head -c 100000 /dev/zero | tr "\0" "$c"; printf "\0"; done > "$d/a"; got=$(/opt/fletch/bin/fletch-guest exec --env-file "$d/e" --args-file "$d/a" -- /bin/sh -c "for x; do printf \"%s \" \"\${#x}\"; done" sh) && [ "$got" = "100000 100000 100000 " ] && [ ! -e "$d/a" ] && echo "3 arguments of 100000 bytes each arrive whole; the file is gone"'
check T-mkfs "mkfs.ext4 and e2fsck for the data disk" -- \
  runroot sh -c 'mkfs.ext4 -V 2>&1 | head -1 && e2fsck -V 2>&1 | head -1'
# sudo is setuid: what makes it root is the overlay honouring that bit.
check T-sudo "agent runs sudo -n without a password, and as root writes the system through its overlay" -- \
  agent_sh 'set -e; test "$(sudo -n id -u 2>/dev/null)" = 0; sudo -n sh -c "echo x > /usr/local/share/t && rm /usr/local/share/t"; echo "sudo -n is root; /usr is writable"'
check T-docker-proxies "forget-0.5.0-docker-proxies takes back the docker client proxies image 0.5.0 wrote, and only those" -- \
  forgets_only_what_0_5_0_wrote
check T-apt "apt reaches its https sources through the egress proxy" -- \
  runroot sh -c 'set -e; apt-config dump | grep -qF "Acquire::https::Proxy \"http://127.0.0.1:8899\";"; apt-config dump | grep -qF "Acquire::http::Proxy \"http://127.0.0.1:8899\";"
schemes=$(apt-get update --print-uris | cut -c2-9 | sort -u); test "$schemes" = https://; echo "proxy 127.0.0.1:8899; every source $schemes"'
check T-guest-listeners "fletch-guest listeners prints a loopback server when it starts and again when it stops" -- \
  agent_sh 'node -e "setTimeout(() => { const s = require(\"net\").createServer().listen(39123, \"127.0.0.1\"); setTimeout(() => s.close(), 1500); }, 1200)" &
out=$(timeout 5 /opt/fletch/bin/fletch-guest listeners); echo $out; echo "$out" | grep -q 39123 && ! echo "$out" | tail -1 | grep -q 39123'
check T-guest-reclaim "sudo -n fletch-guest reclaim, as the host runs it, exits 0" -- \
  agent_sh 'sudo -n /opt/fletch/bin/fletch-guest reclaim 2>&1 && echo reclaimed'
check T-nollvm-files "Mesa/LLVM files are excluded; libgbm stays" -- \
  sh -c "! ls $LIB/libLLVM* $LIB/libgallium* $LIB/libz3* 2>/dev/null && ls $LIB/libgbm.so.1 && echo 'libgbm present, backend absent'"
check T-locale "no /usr/share/locale; C.UTF-8 works" -- \
  agent_sh 'test ! -d /usr/share/locale/de && python3 -c "import sys; print(sys.getfilesystemencoding())"'
check T-fonts "fontconfig resolves sans-serif, emoji and CJK on the base fonts" -- \
  agent_sh 'fc-match sans-serif && fc-match emoji && fc-match "sans-serif:lang=zh" && fc-list | wc -l'
check T-dev-gcc "devtools: gcc compiles and runs a C program" -- \
  agent_sh 'cd $(mktemp -d) && printf "#include <stdio.h>\nint main(){puts(\"hello from gcc\");return 0;}\n" > h.c && gcc -O2 h.c -o h && ./h && g++ --version | head -1'
check T-dev-bun "devtools: bun runs a script and bunx is beside it" -- \
  agent_sh 'cd $(mktemp -d) && echo "console.log(\"hi from bun \" + Bun.version)" > h.ts && bun run h.ts && test -L /usr/local/bin/bunx && bunx --version'
check T-dev-python "devtools: python3 with ssl, sqlite3, pip and venv" -- \
  agent_sh 'python3 -c "import ssl, sqlite3, json; print(\"py ok\")" && pip --version && python3 -m venv /tmp/venv && /tmp/venv/bin/python -c "print(\"venv ok\")"'
check T-ldd "no missing shared libraries for python3, gcc, node, git, sshd, rg through the overlay" -- \
  runroot sh -c 'for b in /usr/bin/python3 /usr/bin/gcc /usr/bin/git /usr/sbin/sshd /usr/local/bin/node /usr/bin/rg; do ldd $b | grep -q "not found" && { echo "$b: missing libs"; ldd $b | grep "not found"; exit 1; }; done; echo "all resolve"'
check T-dev-nodegyp "devtools: node-gyp builds a native addon against the Node headers in the layer" -- \
  agent_sh 'set -e; cd $(mktemp -d); test -f /usr/local/include/node/node_api.h; npm init -y >/dev/null; npm i --silent node-gyp@latest node-addon-api@latest
INC=$(node -p "require(\"node-addon-api\").include_dir")
printf "{ \"targets\": [ { \"target_name\": \"hello\", \"sources\": [ \"hello.cc\" ], \"include_dirs\": [ \"%s\" ], \"defines\": [ \"NAPI_DISABLE_CPP_EXCEPTIONS\" ] } ] }\n" "$INC" > binding.gyp
printf "#include <napi.h>\nNapi::String Hi(const Napi::CallbackInfo& i){return Napi::String::New(i.Env(),\"hi from addon\");}\nNapi::Object Init(Napi::Env e, Napi::Object x){x.Set(\"hi\",Napi::Function::New(e,Hi));return x;}\nNODE_API_MODULE(hello, Init)\n" > hello.cc
npx node-gyp rebuild --nodedir=/usr/local >/dev/null 2>&1 || npx node-gyp rebuild --nodedir=/usr/local
node -e "console.log(require(\"./build/Release/hello.node\").hi())"'
check T-no-browser-layer "no system browser without the browser layer" -- \
  sh -c "! test -e $M/usr/bin/chromium-headless-shell && ! test -e $M/usr/bin/chromium && echo absent"
check T-playwright "Playwright downloads chrome-headless-shell and screenshots on the base libraries" -- \
  agent_sh 'set -e; cd $(mktemp -d); npm init -y >/dev/null; npm i --silent playwright; npx playwright install chromium-headless-shell 2>&1 | tail -2; cat > s.mjs <<EOF
import { chromium } from "playwright";
const b = await chromium.launch({ args: ["--no-sandbox", "--disable-dev-shm-usage"] });
const p = await b.newPage();
await p.setContent("<h1 style=\"font:40px sans-serif\">Hello 你好 Привет 😀</h1><canvas id=c></canvas><script>c.getContext(\"2d\").fillRect(0,0,50,50)</script>");
await p.screenshot({ path: "shot.png" });
await b.close();
EOF
node s.mjs && stat -c "%s bytes" shot.png && test $(stat -c %s shot.png) -gt 2000'

# overlay cost: a cold recursive scan of /usr, all layers vs the plain base
drop_caches() { sync; echo 3 > /proc/sys/vm/drop_caches 2>/dev/null || true; }
# shellcheck disable=SC2086
assemble $(echo $layers | tr ' ' '\n' | tac | tr '\n' ' ')
drop_caches
t0=$(date +%s%N); find "$M/usr" -printf '' ; t1=$(date +%s%N)
merged_ms=$(( (t1 - t0) / 1000000 ))
drop_caches
t0=$(date +%s%N); find "$MNT/base/usr" -printf '' ; t1=$(date +%s%N)
base_ms=$(( (t1 - t0) / 1000000 ))
if [ "$base_ms" -gt 0 ] && [ $(( merged_ms * 10 )) -le $(( base_ms * 40 )) ]; then
  pass T-overlay-perf "cold find over /usr: base ${base_ms} ms, overlay ${merged_ms} ms"
else
  fail T-overlay-perf "cold find over /usr: base ${base_ms} ms, overlay ${merged_ms} ms (more than 4x)"
fi

# ------------------------------------------------------------ every layer
say "configuration: every layer"
check T-browser "browser layer: chromium-headless-shell screenshots text, emoji and a canvas; no Mesa/LLVM loaded" -- \
  agent_sh "set -e; cd \$(mktemp -d); cat > p.html <<'EOF'
$PAGE
EOF
LD_DEBUG=libs chromium-headless-shell --no-sandbox --disable-dev-shm-usage --window-size=600,200 --screenshot=s.png file://\$PWD/p.html 2>ld.txt >/dev/null
test \$(stat -c %s s.png) -gt 2000
! grep -E 'calling init: .*(gallium|LLVM|libz3|libGL|gtk)' ld.txt
echo \"\$(stat -c %s s.png) byte png; \$(grep -c 'calling init' ld.txt) libs loaded; \$(chromium-headless-shell --version 2>/dev/null | head -1)\""
check T-fontcache "no fontconfig cache rebuilt into HOME by the system browser" -- \
  agent_sh 'n=$(ls ~/.cache/fontconfig 2>/dev/null | wc -l); echo "$n cache files in HOME"; test "$n" -eq 0'
check T-fonts-noto "browser layer adds fonts-noto-core (Devanagari resolves to Noto)" -- \
  agent_sh 'm=$(fc-match "sans-serif:lang=hi"); echo "lang=hi -> $m"; echo "$m" | grep -i -E "noto|devanagari"'
check T-ldd-browser "the browser resolves every library through the overlay" -- \
  runroot sh -c 'b=$(readlink -f /usr/bin/chromium-headless-shell); ldd "$b" | grep "not found" && exit 1; echo "all resolve"'
check T-containers "containers layer: docker, compose, buildx, containerd, runc, nft; dockerd takes daemon.json" -- \
  runroot sh -c 'set -e; export HOME=/tmp; docker --version; docker compose version; docker buildx version; containerd --version; runc --version | head -1; nft --version
dockerd --validate --config-file /etc/docker/daemon.json'
check T-containers-wiring "agent is in the docker group; containers resolve names at the bridge gateway daemon.json sets" -- \
  runroot sh -c 'set -e; id -nG agent | grep -qw docker
gw=$(jq -r .bip /etc/docker/daemon.json | cut -d/ -f1); test "$(jq -c .dns /etc/docker/daemon.json)" = "[\"$gw\"]"
echo "docker group: $(id -nG agent); dns $gw"'
# The image's sentinel-dns as a resolver asks it; what containers do with the
# answer is boot-check.sh's.
check T-containers-dns "sentinel-dns: a dotted name is the sentinel and has no AAAA; a single-label name is NXDOMAIN" -- \
  sh -c 'set -e; "$1" sentinel-dns 127.0.0.1:5353 198.18.0.1 >/dev/null & trap "kill $!" EXIT; sleep 0.3
q() { dig +time=2 +tries=1 -p 5353 @127.0.0.1 "$@"; }
a=$(q +short registry.npmjs.org A); test "$a" = 198.18.0.1; test -z "$(q +short registry.npmjs.org AAAA)"
q web A | grep -q "status: NXDOMAIN"; echo "registry.npmjs.org A $a, no AAAA; web NXDOMAIN"' sh "$MNT/base/opt/fletch/bin/fletch-guest"
# A package two layers both pull in puts the same file in each, which is harmless.
layers_agree() {
  local l differ
  differ=$(for l in $layers; do
    (cd "$MNT/$l" && find . -type f ! -name .fletch-layer -exec sha256sum {} +)
  done | sort -u | awk '{ print $2 }' | sort | uniq -d)
  [ -z "$differ" ] || { echo "$differ"; return 1; }
  echo "no file differs between layers"
}
check T-layers-disjoint "no file is in two layers with different content, where the one on top would hide the other's" -- layers_agree

# ------------------------------------------------------------ base only: the agent CLIs
say "configuration: base only"
assemble
check T-base-only-no-dev "gcc, python3, bun and docker are absent on the base alone" -- \
  sh -c "! test -e $M/usr/bin/gcc && ! test -e $M/usr/bin/python3 && ! test -e $M/usr/local/bin/bun && ! test -e $M/usr/bin/dockerd && echo absent"
if [ "$WITH_AGENTS" = 1 ]; then
  check T-agents-claude "Claude Code installs and starts on the base" -- \
    agent_sh 'set -e; P=/tmp/t/claude; mkdir -p $P; npm install -g --prefix $P --cache /tmp/npm-cache --no-fund --no-audit --silent @anthropic-ai/claude-code@latest; $P/bin/claude --version'
  check T-agents-codex "Codex installs and starts on the base" -- \
    agent_sh 'set -e; P=/tmp/t/codex; mkdir -p $P; npm install -g --prefix $P --cache /tmp/npm-cache --no-fund --no-audit --silent @openai/codex@latest; $P/bin/codex --version'
  check T-agents-opencode "OpenCode installs and starts on the base" -- \
    agent_sh 'set -e; P=/tmp/t/opencode; mkdir -p $P; npm install -g --prefix $P --cache /tmp/npm-cache --no-fund --no-audit --silent opencode-ai@latest; $P/bin/opencode --version'
  check T-agents-pi "Pi installs and starts on the base (needs the system Node)" -- \
    agent_sh 'set -e; P=/tmp/t/pi; mkdir -p $P; npm install -g --prefix $P --cache /tmp/npm-cache --no-fund --no-audit --silent @earendil-works/pi-coding-agent@latest; $P/bin/pi --version'
  check T-agents-cursor "Cursor CLI installs (glibc, libstdc++, bash) and starts on the base" -- \
    agent_sh 'set -e; P=/tmp/t/cursor; mkdir -p $P; curl -fsSL https://cursor.com/install -o /tmp/cursor-install.sh; HOME=$P bash /tmp/cursor-install.sh >/dev/null 2>&1 || HOME=$P bash /tmp/cursor-install.sh; $P/.local/bin/cursor-agent --version'
else
  row T-agents SKIPPED "WITH_AGENTS=0"
fi
teardown

{
  echo
  echo "## Sizes"
  echo
  cat "$OUT/sizes.md"
} >> "$REPORT"
say "report: $REPORT"
exit $failed
