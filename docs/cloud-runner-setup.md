# The cloud runner: what the engine sets up, and where it lives

A runner is `fletch-host` on a machine at the user's cloud provider: always
on, supervising agents with no laptop online, each workspace in a provider
machine of its own, and serving every paired device (desktop or phone)
through the relay. The design is `docs/cloud-runner.md` (the spec); this page
is what `crates/fletch-core/src/sandbox/cloud_runner` does today. The desktop's
Settings calls its functions (`status`, `create`, `start`, `stop`, `delete`,
`pairing_link`); one runner per install.

## On this machine

The runner's directory is `<checkouts base>/cloud-runner`
(`~/.fletch/cloud-runner`, `~/.fletch/dev/cloud-runner` from a debug build),
beside the boxes root and never inside it, so no sweep of boxes sees it. It
holds what a cloud box's directory holds, and a record once the runner is set
up:

| File | What |
| --- | --- |
| `key`, `key.pub` | the ssh key pair the runner's sshd trusts (`FLETCH_BOX_AUTHORIZED_KEY`) |
| `known_hosts` | the runner's ssh host key, read through the provider's exec and pinned |
| `cloud` | the handle of its machine and disk at the provider |
| `ssh_config`, `cm.sock` | the channel's config, and its master's socket while a step is connected |
| `runner.json` | the record: the runner's host public key (`hostId`), its `name` and its `relay` |

## The machine

The cloud image a box runs (`sandbox::boxes::cloud::image()`, or
`FLETCH_CLOUD_IMAGE`), a box's default size (2 vCPUs, 2048 MiB, an 8 GiB
disk; what a runner needs is unmeasured), and a disk at `/data` that outlives
the machine. Its provider keeps it running (`MachineSpec::keep_running`: the
`on-failure` restart policy on the first provider), and a stop asked for stays
a stop. Its environment is the entrypoint's contract and nothing secret:

| Env | Value |
| --- | --- |
| `FLETCH_BOX_HOME` | `/home/agent`, bound from `/data/home`: the service's home |
| `FLETCH_BOX_RPC_DIR` | `/home/agent/.fletch-runner/rpc` (required, unused) |
| `FLETCH_BOX_WRITABLE_ROOT` | `/home/agent/.fletch-runner/ws` (required, unused) |
| `FLETCH_BOX_AUTHORIZED_KEY` | `key.pub` above |
| `FLETCH_BOX_ID` | `runner` |
| `FLETCH_BOX_SSH_PORT` | `2222` |

The home is not under `/data` on purpose: the runner's home and the
directories in its data directory are the identity paths of its workspaces'
boxes, and a box's entrypoint refuses an identity path under `/data`.

## On the runner's disk

```
/data/host/                 0700, the agent's
  fletch-host               the release of the engine's version (its tarball's
                            other files beside it: the box runtime, README.md)
  env                       CLAUDE_CODE_OAUTH_TOKEN=…   0600
  start                     the start script the entrypoint runs
  start.pid                 the running start script's pid, while it runs
/data/tools/claude/         the Claude CLI; the entrypoint binds /data/tools
                            read-only at /opt/fletch/tools
/data/home/                 the service's home, bound at /home/agent
  fletch-host/              its data directory (--data-dir /home/agent/fletch-host):
                            data.db (workspaces, sessions, the GitHub token and
                            the provider connection in its settings table),
                            remote/host_key (its identity), remote/devices.json
                            (its pairings), host.sock, workspaces/, rpc/, boxes/
/data/ssh/                  the machine's ssh host key, the laptop's key
```

The start script, as the engine writes it (relay and name quoted):

```sh
#!/bin/sh
export HOME=/home/agent USER=agent LOGNAME=agent SHELL=/bin/bash LANG=C.UTF-8
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
for bin in /opt/fletch/tools/*/bin; do
  [ -d "$bin" ] && PATH=$bin:$PATH
done
export PATH
set -a
. /data/host/env
set +a
cd "$HOME" || exit 1
stopping=
service=
trap 'stopping=1; [ -n "$service" ] && kill -TERM "$service" 2>/dev/null' TERM
echo $$ > /data/host/start.pid
while [ -z "$stopping" ]; do
  /data/host/fletch-host serve --data-dir /home/agent/fletch-host --relay wss://relay.fletch.sh --name 'Fletch runner' &
  service=$!
  status=0
  while kill -0 "$service" 2>/dev/null; do
    [ -n "$stopping" ] && kill -TERM "$service" 2>/dev/null
    wait "$service"
    status=$?
  done
  service=
  [ -n "$stopping" ] && break
  echo "fletch-host exited with status $status; starting it again in 5 s" >&2
  sleep 5
done
rm -f /data/host/start.pid
```

The service runs in the background of the script, which waits for it: when
it exits on its own, the script starts it again after five seconds; when the
script itself is sent TERM, it passes the TERM to the service, waits for it
to end, removes its pid file and ends without starting it again.

Its flags live in the script, not in the image: a machine keeps the image it
was made with. The image's entrypoint runs `/data/host/start` when the disk
holds one, at every start of the machine, as `agent`, in the background, with
an empty environment, before it execs sshd (`packaging/cloud-image/README.md`).

## Creating it

`create(db, provider, inputs, progress)` refuses while a runner exists. Then,
each step reported to `progress`:

1. **The machine**: made through the adapter with `machine_up`, its host key
   pinned, then reached over ssh (`reach`).
2. **`fletch-host`**: the engine fetches the release's
   `fletch-host-<version>-<target>.tar.gz.sha256` itself over HTTPS, for its
   own version and the guest's architecture (`uname -m`); the guest downloads
   the tarball, checks it against that digest with `sha256sum`, and unpacks it
   into `/data/host`. `FLETCH_CLOUD_RUNNER_BINARY` names a local `fletch-host`
   to push instead, for a development build, which has no release: it must be
   built for the runner's machine (Linux x86-64, glibc), and with
   `QUORUM_GITHUB_CLIENT_ID` set at build time for the GitHub step to work, as
   the release workflow builds it. The binary's minisign signature is not
   checked at this first install (`fletch-host update` checks it for every
   later one).
3. **The Claude CLI**: `fletch_box::tools::install_cli` at the version this
   engine launches (`registry::tools_version`): a host with no agent CLI
   reports its provider not installed, and clients start no agent of it.
4. **The env file** with the Claude token (`claude setup-token`, from the
   caller), written as a file's content over ssh, never on a command line, and
   **the start script**.
5. **The service**: the machine is stopped and started through the adapter, so
   the image's hook starts the service as it will at every start, and the
   engine waits for its admin socket. The service has never run before this
   stop, so the stop is the provider's alone. An image without the hook fails
   with a message that says so.
6. **Over the admin socket** (`fletch-host call <op>` over ssh, the arguments
   on stdin): `cloud_connect` with this machine's own provider connection
   (which, on the runner, makes its default spare machine in the background),
   `add_device` with the calling device's public key and full access,
   `github_login_start` — the caller shows the code and the URL — and
   `github_login_status` until it is entered, then `status` for the runner's
   identity, kept as `runner.json`.

A failure at any step destroys the machine and its disk and removes the
directory, so a retry starts clean.

## A cold start with no laptop online

The provider starts the machine (or restarts it after its main process
failed). The entrypoint binds `/data/tools`, `/data/home` and the other two
paths, writes the laptop's key, starts `/data/host/start` as the agent and
execs sshd. The script loads the Claude token, puts the CLIs on its `PATH` and
runs `fletch-host serve`, which opens its database on the disk, keeps its host
key (the same identity) and its paired devices, reads its GitHub token and
provider connection from its settings table, stops any workspace machine left
running, brings its spare machines to their number, and dials the relay.
Devices reach it there; a new workspace gets a machine at the connected
provider (a spare when one is ready), since the runner cannot boot a VM.

## The rest of its life

- `status` reads the directory and asks the provider: none, being set up, a
  set-up that did not finish, or the machine's state (running, stopped,
  machine lost, disk lost, failed with the reason), with the record.
- `start` brings the machine up with `machine_up`: a lost machine is made
  again on the runner's disk with the same spec, the same runner; a lost disk
  is refused and never made again on an empty one.
- `stop` ends the service cleanly before the machine goes down. Over ssh, it
  sends TERM to the start script whose pid is in `/data/host/start.pid` (and
  only while that pid is a process running the script), waits up to 90 s for
  it to be gone, and syncs the disk; then the provider stops the machine. The
  script passes the TERM to `fletch-host`, whose own shutdown (the one a TERM
  gives every `fletch-host serve`) kills its agents' sessions and stops each
  running workspace machine: it pulls the box's sessions, syncs its disks and
  has the provider stop it, each within ten seconds, the machines side by
  side. So a stopped runner leaves none of its workspaces' machines running.
  A machine that is not running has nothing to end: the provider's stop is
  all. When the guest cannot be reached over ssh, or the service does not end
  within the bound, the machine is stopped all the same, so a runner that
  cannot be ended keeps nobody billed, and that is logged as a warning;
  SQLite's own journal is then what protects the databases.
- `pairing_link(preset)` asks the running runner for a pairing code for
  another device (`begin_pairing`) and answers its link, code and expiry.
- `delete(force)` has the runner destroy every machine it made
  (`destroy_cloud_boxes`; the runner is started for it): its spare machines
  first, after which its process makes no new spare (a replenish under way is
  waited for), then every workspace's machine. Then it destroys its own
  machine and disk and removes the directory. A runner that cannot be reached
  (its disk lost, a machine that will not start), or that could not destroy
  them all (a spare or a workspace's machine its provider would not delete),
  is refused unless `force`, and with `force` the answer says which machines
  may remain at the provider. A deletion needs no clean end of the service:
  the disk goes with the machine.

**Disconnecting the provider** the runner is at is refused on this machine
(`cloud_disconnect`, the engine's `commands::cloud_disconnect_impl`) while
the runner exists, decided by the runner's directory and handle alone: the
runner is billed while it exists and can only be managed through that
connection, so it is deleted first. The runner itself has no runner of its
own, so the rule changes nothing there; its own disconnect destroys its
spares first, as on any host.

`fletch-host update`, run on the runner, replaces `/data/host/fletch-host` in
place; the start script runs the new binary once the old one exits.
