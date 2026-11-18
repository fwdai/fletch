#!/bin/bash
# Test a cloud image the way the engine reaches one: run it with the four
# variables, a volume at /data and sshd's port published on loopback, read
# the host key through `docker exec` (a provider's exec, for a machine), and
# connect from this machine's ssh with a generated key and that host key
# pinned under the `box` alias, in a config of the shape the channel writes
# for a TCP route (crates/fletch-box/src/channel, Route::tcp). Then check
# what the engine and the agents rely on, stop and start the container, and
# check that /data and the host key outlived it. Needs docker (privileged
# containers: the entrypoint mounts), ssh, ssh-keygen and the network.
#
#   test.sh IMAGE [OUT_DIR]
#
# Writes OUT_DIR/report.md (one row per check) and OUT_DIR/test.log, and exits
# non-zero when a check failed.
set -u

IMAGE=${1:?usage: test.sh IMAGE [OUT_DIR]}
OUT=${2:-$(mktemp -d)}
OUT=$(cd "$OUT" && pwd)
REPORT=$OUT/report.md
LOG=$OUT/test.log
: > "$LOG"
failed=0
T=$(mktemp -d)
C=fletch-cloud-test-$$
# The host paths a Mac would pass: the mailbox and the writable root in HOME.
H=/Users/tester
WS=$H/.fletch/workspaces/t1
RPC=$H/.fletch/rpc/t1

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
cleanup() {
  docker logs "$C" >> "$LOG" 2>&1
  docker rm -f -v "$C" >/dev/null 2>&1
  rm -rf "$T"
}
trap cleanup EXIT
{
  echo "## Checks: $IMAGE"
  echo
  echo "| Check | Result | Note |"
  echo "| --- | --- | --- |"
} > "$REPORT"

ssh-keygen -q -t ed25519 -N '' -C fletch-test -f "$T/key"
# up: waits until the container's sshd answers on its published port (another
# one after each start), with the config pointed there. The host key is read
# through docker exec the first time and pinned for good.
up() {
  local port
  for _ in $(seq 1 100); do
    [ "$(docker inspect -f '{{.State.Running}}' "$C")" = true ] || { echo "the container exited"; return 1; }
    port=$(docker port "$C" 2222/tcp 2>/dev/null | head -n 1)
    port=${port##*:}
    if [ -n "$port" ] && [ ! -s "$T/known_hosts" ]; then
      key=$(docker exec "$C" cat /data/ssh/host_key.pub 2>/dev/null) &&
        read -r keytype base64 _ <<< "$key" && echo "box $keytype $base64" > "$T/known_hosts"
    fi
    if [ -n "$port" ] && [ -s "$T/known_hosts" ]; then
      cat > "$T/config" <<EOF
Host box
  User agent
  HostKeyAlias box
  IdentityFile "$T/key"
  IdentitiesOnly yes
  IdentityAgent none
  ForwardAgent no
  BatchMode yes
  StrictHostKeyChecking yes
  UpdateHostKeys no
  UserKnownHostsFile "$T/known_hosts"
  GlobalKnownHostsFile /dev/null
  HostName 127.0.0.1
  Port $port
  ConnectTimeout 10
  ServerAliveInterval 15
  ServerAliveCountMax 3
  LogLevel ERROR
EOF
      ssh -F "$T/config" box true 2>/dev/null && { echo "sshd answers on 127.0.0.1:$port"; return 0; }
    fi
    sleep 0.3
  done
  echo "sshd did not answer within 30 s"
  return 1
}
# g SCRIPT: runs SCRIPT in the guest as the agent, through the login shell.
g() { ssh -F "$T/config" box "$1"; }
# is EXPECTED COMMAND...: the command prints EXPECTED.
is() {
  local expected=$1 out; shift
  out=$("$@") || return 1
  [ "$out" = "$expected" ] || { echo "printed '$out', not '$expected'"; return 1; }
  echo "$out"
}
# The launch path, word for word as Channel::remote_command quotes it, with
# the env file written over stdin as Channel::put writes it.
launch() {
  printf 'HOME=%s\nPATH=/usr/local/bin:/usr/bin:/bin\nFLETCH_T=from-the-env-file\n' "$H" |
    g 'umask 077 && cat > /data/run/t.env && chmod 600 /data/run/t.env' || return 1
  ssh -F "$T/config" box /opt/fletch/bin/fletch-guest exec --env-file /data/run/t.env --cwd "$WS" \
    -- sh -c "'echo \$PWD \$FLETCH_T'"
}
restarted() {
  up >/dev/null || return 1
  [ "$(docker exec "$C" cat /data/ssh/host_key.pub)" = "$key_before" ] || { echo "the host key changed"; return 1; }
  is kept g "cat $WS/kept" >/dev/null || { echo "$WS/kept did not outlive the stop"; return 1; }
  echo "the same host key, $WS/kept still there"
}

# ------------------------------------------------------------ the contract's refusals
# refuses EXPECTED VAR=VALUE...: the entrypoint, with these variables over
# good ones, prints FLETCH_INIT_ERROR EXPECTED and exits non-zero.
refuses() {
  local expected=$1 out; shift
  local env=("FLETCH_BOX_HOME=$H" "FLETCH_BOX_WRITABLE_ROOT=$WS" "FLETCH_BOX_RPC_DIR=$RPC" "FLETCH_BOX_AUTHORIZED_KEY=$(cat "$T/key.pub")" "$@")
  local args=() e
  for e in "${env[@]}"; do args+=(-e "$e"); done
  if out=$(docker run --rm --privileged "${args[@]}" "$IMAGE" 2>&1); then
    echo "exited 0 with $*"; return 1
  fi
  echo "$out" | grep -qxF "FLETCH_INIT_ERROR $expected" || { echo "with $*: $out"; return 1; }
  echo "FLETCH_INIT_ERROR $expected"
}
check T-env "the entrypoint refuses a missing variable" -- \
  refuses "FLETCH_BOX_AUTHORIZED_KEY is not set in the environment" FLETCH_BOX_AUTHORIZED_KEY=
check T-env-home "the entrypoint refuses a HOME inside the writable root" -- \
  refuses "FLETCH_BOX_HOME may not lie inside the writable root or the rpc dir" "FLETCH_BOX_HOME=$WS/h"
check T-env-paths "the entrypoint refuses a host path under the guest's own directories" -- \
  refuses "cannot place /etc/ws under /etc: it would shadow the guest's own files" FLETCH_BOX_WRITABLE_ROOT=/etc/ws

# ------------------------------------------------------------ a running box
say "starting $IMAGE"
docker run -d --privileged --name "$C" -v /data -p 127.0.0.1::2222 \
  -e FLETCH_BOX_ID=test -e "FLETCH_BOX_HOME=$H" -e "FLETCH_BOX_WRITABLE_ROOT=$WS" -e "FLETCH_BOX_RPC_DIR=$RPC" \
  -e "FLETCH_BOX_AUTHORIZED_KEY=$(cat "$T/key.pub")" "$IMAGE" >> "$LOG" ||
  { fail T-start "docker run failed"; exit 1; }
check T-ssh "ssh reaches sshd on 2222 with the host key read through exec, pinned under the box alias" -- up ||
  exit 1
check T-user "sessions are the agent's" -- is agent g whoami
# Port 2222 (08AE) in the listen state (0A), on the IPv4 and the IPv6 wildcard.
check T-listen "sshd listens on every address, IPv4 and IPv6" -- \
  docker exec "$C" sh -c 'grep -q "^ *[0-9]*: 00000000:08AE 00000000:0000 0A" /proc/net/tcp &&
grep -q "^ *[0-9]*: 00000000000000000000000000000000:08AE 00000000000000000000000000000000:0000 0A" /proc/net/tcp6 && echo "0.0.0.0:2222 and [::]:2222"'
check T-identity "the host paths exist, belong to agent, resolve to themselves and are /data's trees" -- \
  g "set -e; for p in $H $RPC $WS $H/.fletch $H/.fletch/workspaces; do
  test -d \$p; test \$(stat -c %U \$p) = agent; test \$(realpath \$p) = \$p; done
for p in $H $RPC $WS; do mountpoint -q \$p; done
echo x > $WS/.t; test \$(cat /data/ws/.t) = x; rm $WS/.t
echo 'three binds of /data, owned by agent'"
check T-tools "/opt/fletch/tools shows /data/tools, read-only" -- \
  g 'set -e; mkdir -p /data/tools/t/bin; printf "#!/bin/sh\necho tool ok\n" > /data/tools/t/bin/t; chmod 755 /data/tools/t/bin/t
! touch /opt/fletch/tools/t/x 2>/dev/null; /opt/fletch/tools/t/bin/t'
check T-exec "fletch-guest exec runs a command with the env file in the writable root" -- \
  is "$WS from-the-env-file" launch
check T-exec-confined "a confined launch (/data covered) runs the provider CLIs and cannot write the home" -- \
  g "set -e; out=\$(/opt/fletch/bin/fletch-guest exec --env-file /data/run/t.env --read-only $H -- sh -c 'ls -A /data; /opt/fletch/tools/t/bin/t; touch $H/x 2>/dev/null || echo home read-only')
test \"\$out\" = \"\$(printf 'tool ok\nhome read-only')\"; echo \$out"
# A confined launch makes a user namespace as the guest user and maps its ids
# there. The container runs on this machine's kernel, and Ubuntu's AppArmor
# policy (the default from 24.04) gives such a namespace no capabilities, so
# the launch is refused whatever the image holds.
if tail -1 "$LOG" | grep -q '^== FAIL T-exec-confined' &&
  [ "$(docker exec "$C" cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns 2>/dev/null)" = 1 ]; then
  say "note: this machine's kernel restricts unprivileged user namespaces; run 'sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0' and test again"
fi
check T-git "git init, commit and log in the writable root" -- \
  g "set -e; cd $WS && git init -q r && cd r && git -c user.email=a@b -c user.name=a commit -q --allow-empty -m x && git log --oneline | head -1"
check T-node "node and npm run" -- g 'node -e "console.log(\"node \" + process.version)" && npm -v'
check T-https "outbound https works with no proxy setting" -- \
  g 'set -e; test -z "$(env | grep -i _proxy)"; curl -fsS -o /dev/null -w "%{http_code}" https://registry.npmjs.org/'
check T-apt "apt-get update under sudo, with no proxy configured" -- \
  g 'set -e; test -z "$(apt-config dump | grep -i proxy)"; sudo -n apt-get -qq update; echo "updated, no proxy"'
check T-sudo "sudo is setuid root and the agent's without a password" -- \
  g 'set -e; test $(stat -c %a /usr/bin/sudo) = 4755; test $(sudo -n id -u) = 0; echo "sudo 4755, root"'
check T-docker "no dockerd runs; one started by hand validates daemon.json, without the sentinel DNS or the egress proxy" -- \
  g 'set -e; ! pgrep -x dockerd; ! test -e /run/docker.sock; ! test -e /etc/apt/apt.conf.d/90fletch-proxy
jq -e "has(\"dns\") or has(\"proxies\") | not" /etc/docker/daemon.json >/dev/null; sudo -n dockerd --validate --config-file /etc/docker/daemon.json'
check T-image "the image names itself and the box image it was made from" -- \
  g 'cat /etc/fletch-image /etc/fletch-cloud-image'

# ------------------------------------------------------------ a stop and a start
key_before=$(docker exec "$C" cat /data/ssh/host_key.pub)
g "echo kept > $WS/kept" >> "$LOG" 2>&1
say "stopping and starting the container"
docker stop -t 10 "$C" >> "$LOG" && docker start "$C" >> "$LOG"
check T-restart "after a stop and a start: the host key pinned before, and /data's files" -- restarted
check T-restart-binds "after a start the host paths and the tools are bound again" -- \
  g "set -e; for p in $H $RPC $WS /opt/fletch/tools; do mountpoint -q \$p; done; /opt/fletch/tools/t/bin/t"

say "report: $REPORT"
exit $failed
