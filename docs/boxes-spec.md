# Boxes: implementation spec

Status: approved design, not yet implemented. This is the build spec for the
box engine described in `docs/boxes.md`. Read that document first: it carries
the model, the rationale and five dated spike findings this spec depends on.
Where the two disagree, this spec wins for *how*, `docs/boxes.md` wins for
*why*. The spike's throwaway code under `spike/boxes/` is a working reference
for every host-guest interaction below; Calina (`~/Code/calina`) is the
reference for the runtime layer (`src-tauri/src/sandbox/`, `src-tauri/src/bin/
calina-vm/`, `sandbox/libkrun/`, `sandbox/image/`).

## 1. Goal and constraints

Add a fourth sandbox engine, **Box**, that runs each agent in a libkrun
micro-VM with no shared folder and no network device, reachable from the host
only over SSH on vsock and an egress proxy. It must:

1. **Hide behind a feature flag** toggled on the Developer Settings pane. With
   the flag off, nothing about the product changes: the Sandbox settings page
   shows the three current engines, no box code runs, no download happens.
2. **Run in parallel with seatbelt, Docker and Podman.** Those engines are not
   modified except where a shared seam is introduced as a pure refactor. An
   agent is stamped with its engine at creation and keeps it, as today.
3. **Fail closed.** A box-stamped agent whose runtime, image or helper is
   missing does not launch; it reports why. It never falls back to seatbelt.
4. **Preserve path identity.** Inside the guest, the agent's writable root, its
   RPC mailbox and its home directory sit at the *same absolute paths* as on
   the host, the way the Docker engine does (its invariant 1). Transcripts,
   RPC payloads and diff paths embed absolute paths, and nothing in the host
   translates them.
5. **Keep the host contract small.** Everything the host does to a box goes
   through one `Channel` (SSH) plus one `EgressProxy`. No bespoke protocol.

Non-goals for this spec: the host runner for macOS and iOS targets, the
multi-host preview forward, Linux-host KVM support, Intel Macs, Windows.
They are listed in §12 so nothing here forecloses them.

## 2. Vocabulary

| Term | Meaning |
| --- | --- |
| Host | The machine running the Fletch engine: desktop app or `fletch-host`. |
| Guest | The Linux system inside one box. |
| Box | One workspace's VM: its disk, sockets, key and process. One box per agent workspace, started on demand. |
| Base image | Read-only Debian root filesystem, erofs, downloaded once per install, attached to every VM. |
| Tools disk | Read-only ext4 disk with the provider CLIs; built by the host from the base image on first use of a provider version. |
| Template disk | Per project, per lockfile hash: ext4 data disk with the repository cloned and setup run. |
| Workspace disk | APFS clone of a template (or an empty sparse disk when no template exists), owned by one box. |
| Channel | The SSH connection family to one box: exec, PTY, sftp, git transport, port forward. |
| Helper | `fletch-vm`: the only binary signed with the hypervisor entitlement; one process per VM; loads libkrun at runtime. |
| Guest agent | `fletch-guest`: a static binary in the image: vsock listener for sshd, mailbox relay, transcript tail, exec wrapper. |

## 3. Components and ownership

```
crates/fletch-box/                 new crate, no fletch-core dependency
  src/runtime/   RuntimePaths, VmSpec, LaunchConfig, Vm (start/stop/kill/wait), console
  src/disks/     base image attach, sparse data disk, APFS clone, dirty marker
  src/image_store/  pinned download, hash verify, versions, prune
  src/egress/    EgressPolicy, EgressProxy (unix socket), EgressEvent, DNS pool
  src/channel/   Channel (OpenSSH client over ProxyCommand), Exec, Pty, Sftp, Forward
  src/guest.rs   the guest contract: ports, paths, env names, fletch-guest subcommands
  src/bin/fletch-vm/   the helper (port of calina-vm)
  src/bin/fletch-box/  a CLI for manual testing: boot, ssh, stop, template, image

crates/fletch-guest/               new crate, builds a static aarch64-linux-musl binary
  vsock-listen, relay, tail, exec, init helpers (see §6.4)

packaging/box-image/               Debian image pipeline (see §6)
packaging/box-bundle/              Tauri hooks: sign fletch-vm, place dylibs (port of Calina's sandbox/bundle)

crates/fletch-core/src/sandbox/boxes/   the engine: BoxEngine, BoxRegistry, provisioning, relays, mirror
crates/fletch-core/src/exec.rs          path-routed Executor seam (§7.1)
```

`fletch-box` depends on nothing in Fletch so it can be tested alone and the
helper binary stays tiny. `fletch-core` depends on `fletch-box`.

## 4. Feature flag and settings

| Key | Where | Values | Meaning |
| --- | --- | --- | --- |
| `boxes_enabled` | host setting (`settings` table), listed in `HOST_SETTING_KEYS` | `true`/`false`, default absent = off | Shows `box` in the Sandbox engine select and allows stamping new agents with it. Toggled only from the Developer pane. A release build honours it if set, since the Developer pane only ships in dev builds and the user wants to live on it. |
| `sandbox_engine` | existing | gains `box` | `EngineKind::from_setting("box")`. Refused by `set_sandbox_engine` when `boxes_enabled` is off. |
| `box_memory_mib` | host setting | default `2048`, min 512 | guest RAM cap per box |
| `box_cpus` | host setting | default `2` | vCPUs per box |
| `box_image_url` | host setting | default empty = GitHub release URL for the pinned version | alternate base URL for the image download (mirrors Calina's `CALINA_IMAGE_URL`) |
| `box_idle_stop_minutes` | host setting | default `10`, `0` = never | stop a box with no attached process after this long |
| `box.egress_allow` | project setting | JSON array of host patterns | per-project additions to the default egress allow-list |

Developer pane: a new row under "Pilots", title "Box sandbox (micro-VM)",
toggle bound to `boxes_enabled` via the existing `set_setting` plumbing. Turning
it off while box agents exist leaves them running and launchable; it only hides
the option for new agents. The Sandbox settings page, when the flag is on, adds
`{ value: "box", label: "Box (micro-VM)" }` to the select and shows the image
install state below it (not installed / downloading n% / version x / damaged),
with Install and Cancel, driven by the events in §9.

Frontend type `SandboxEngine` in `src/storage/preferences` gains `"box"`.
`AgentRecord.sandbox_engine` doc comment lists it.

## 5. Host-side layout

```
<data dir>/box/
  images/<version>/base.erofs(.sha256)      image store (shared by every box)
  tools/<provider>-<cli version>.img        tools disks (read-only once built)
  libkrun/ (only in dev; the bundle has Contents/Frameworks)
~/.fletch/projects/<project id>/box/
  template-<lockfile sha256[..16]>.img      template disks
  template-<...>.json                       what it contains: image version, setup commands, built at
~/.fletch/workspaces/<agent id>/box/
  data.img            the workspace disk (APFS clone of a template or sparse)
  dirty               present while the VM runs; removed after a clean stop
  key, key.pub        the host's ed25519 key for this box
  known_hosts         the guest's host key, learned at first boot from the console
  ssh_config          generated; -F for every ssh/git/sftp invocation
  ssh.sock            libkrun listen-mode socket → guest vsock 22
  egress.sock         host proxy socket ← guest vsock 6000
  console.log         guest console
  mirror.git/         bare repo, alternates → source repos; refs fetched from the guest (§7.4)
  claude-projects/    transcript mirror (same role as Docker's projects_src) (§7.3)
```

Socket paths must stay under 103 bytes (`sun_path`). The checkouts root is
already short; a `fletch-host --data-dir` that is not should fail box creation
with a clear error rather than a truncated path.

## 6. The guest

### 6.1 Image

Debian stable, arm64, built reproducibly to an lz4hc erofs file, pinned in the
app by version, size and sha256 exactly as Calina does (`image_store`). Package
set: `bash`, `coreutils`, `git`, `curl`, `ca-certificates`, `openssh-server`,
`python3`, `python3-venv`, `python3-pip`, `nodejs` 22 (NodeSource or Debian
backports, pinned), `build-essential`, `pkg-config`, `jq`, `ripgrep`,
`unzip`, `socat` (debugging only), `e2fsprogs`, Chromium and fonts in a
**separate optional disk** in a later phase (§11); for the first phase include
them in the base so Playwright works, and split later.

The glibc decision is final (`docs/boxes.md`, phase 2c): Cursor's CLI ships a
glibc Node and glibc native addons; the `gcompat` shim fails on `fcntl64`.

Image contents that are contract, not packages:

- `/opt/fletch/init`: the guest entrypoint (§6.3).
- `/opt/fletch/bin/fletch-guest`: the static guest agent (§6.4).
- `/etc/ssh/sshd_config.fletch`: `AuthorizedKeysFile /data/ssh/authorized_keys`,
  `PasswordAuthentication no`, `PermitRootLogin no`, `AllowUsers agent`,
  `AcceptEnv none`, `AllowTcpForwarding yes`, `GatewayPorts no`,
  `Subsystem sftp /usr/lib/openssh/sftp-server`, `UsePAM no`,
  `PermitUserEnvironment no`, `PrintMotd no`, `HostKey /data/ssh/host_key`.
- A user `agent` (uid 1000, gid 1000, home `/data/home`, shell `/bin/bash`) and
  `root`. The agent runs as `agent`; `IS_SANDBOX` is not needed.
- `/etc/profile.d/fletch.sh` is **not** relied on: every session's environment
  comes from `fletch-guest exec` (§7.2), never from shell profiles.

### 6.2 Disks and mounts

| Device | Content | Mount |
| --- | --- | --- |
| `/dev/vda` | base image, read-only | `/` (erofs, `ro`) via `krun_set_root_disk_remount` |
| `/dev/vdb` | workspace data disk, read-write | `/data` (ext4, `nosuid,nodev,discard,noinit_itable`) |
| `/dev/vdc` | tools disk, read-only | `/opt/fletch/tools` |
| tmpfs | | `/tmp`, `/run`, `/var/tmp` |

Bind mounts for path identity, made by init from values the host passes in
the kernel command line environment (`krun_set_env`, which is safe for
`KEY=VALUE` pairs; only argv is mangled):

| Guest path | Bound from | Host meaning |
| --- | --- | --- |
| `<writable_root>` e.g. `/Users/alex/.fletch/workspaces/<id>` | `/data/ws` | the agent's writable root; checkouts live under it |
| `<rpc_dir>` e.g. `/Users/alex/.fletch/rpc/<id>` | `/data/rpc` | the file mailbox |
| `<home>` e.g. `/Users/alex` | `/data/home` | `$HOME`; `~/.claude`, `~/.codex`, `~/.npm` land here |
| `/opt/fletch/tools/<provider>/bin` on `PATH` | tools disk | the provider CLIs |

`FLETCH_BOX_WRITABLE_ROOT`, `FLETCH_BOX_RPC_DIR`, `FLETCH_BOX_HOME` carry the
three host paths. Init creates the mountpoints under `/` as needed; erofs is
read-only, so init first mounts a small tmpfs over `/Users` (or whatever the
first path component is) and creates the directory tree inside it. The data
disk is formatted by init on first boot exactly as Calina's init does
(formatting 4 MiB short of the device, fsck on failure, refuse if still
unmountable).

### 6.3 Init

`/opt/fletch/init` runs as PID 1's workload under libkrun's own init. In order:

1. Mount tmpfs dirs. Mount `/data` (format or fsck first). Create
   `/data/{ws,rpc,home,ssh,run}` with owner `agent`. Bind-mount per §6.2.
2. Host key: generate `/data/ssh/host_key` if absent (so it persists per
   workspace). Print `FLETCH_HOSTKEY <type> <base64>` on the console; the host
   reads it into `known_hosts` (§7.2).
3. Write `/data/ssh/authorized_keys` from `FLETCH_BOX_AUTHORIZED_KEY` (env).
4. Start `fletch-guest vsock-listen 22 -- /usr/sbin/sshd -i -f /etc/ssh/sshd_config.fletch`
   (one `sshd -i` per connection, no TCP listener at all).
5. Start `fletch-guest egress-loopback 8899 6000` (guest TCP 127.0.0.1:8899 →
   host vsock 6000), so processes that only honour `HTTPS_PROXY` have a
   loopback target.
6. Print `FLETCH_READY` and wait for `FLETCH_STOP` on the control vsock
   (`fletch-guest wait-stop 5000`), then `sync`, unmount `/data`, `poweroff`.

Nothing is passed through argv (libkrun's init re-splits it; spike finding).
Everything init needs is env or on disk.

### 6.4 `fletch-guest` subcommands

| Subcommand | Does |
| --- | --- |
| `vsock-listen <port> -- <cmd...>` | Accept on `AF_VSOCK` port, spawn `cmd` per connection with the socket as stdin/stdout (inetd style). |
| `egress-loopback <tcp port> <vsock port>` | Relay guest loopback TCP to the host vsock port. |
| `exec --env-file <path> [--cwd <dir>] -- <cmd...>` | Clear the environment, load `KEY=VALUE` lines from the file, `chdir`, exec. Used as the remote command for every launch and host exec (§7.2). |
| `relay <mailbox dir>` | Watch `<dir>/requests/*.json` (inotify, 100 ms fallback poll); emit each as one JSON line on stdout `{"op":"request","name":...,"body":...}`; read JSON lines on stdin `{"op":"response","name":...,"body":...}` and write them atomically to `<dir>/responses/`. Delete a request file after emitting it. |
| `tail <dir>` | Watch a directory tree for JSONL files; emit `{"op":"append","path":...,"data":base64}` for new bytes, `{"op":"create"...}` for new files. Used for transcript mirroring. |
| `wait-stop <vsock port>` | Block until the host connects and sends `stop`; used by init. |

Static musl build (`aarch64-unknown-linux-musl`), no runtime dependencies, so
it is independent of the glibc base. Cross-compiled in CI with `cross` or
`cargo zigbuild`; the image build copies it in.

## 7. Host side

### 7.1 The path-routed executor seam (pure refactor, lands before the engine)

Today ~43 call sites resolve `TrackedRepo::checkout_path(agent_id)` and then
run git in it, read files from it or write files into it. Under a box those
paths do not exist on the host, but by path identity they are *exactly the
paths the guest has*. So the seam is not a `Checkout` object every caller must
be rewritten against; it is a router keyed on the path:

```rust
// crates/fletch-core/src/exec.rs
pub enum Executor { Local, Box(Arc<fletch_box::Channel>) }

/// Where a process that works in `dir` must run. Local for every path that is
/// not under a box workspace's writable root; otherwise that box's channel.
pub fn for_path(dir: &Path) -> Executor;   // consults sandbox::boxes::BoxRegistry

impl Executor {
    pub async fn command(&self, program: &str, args: &[&str], cwd: &Path, env: &[(String,String)])
        -> Result<std::process::Output>;
    pub async fn read(&self, path: &Path) -> Result<Vec<u8>>;
    pub async fn write(&self, path: &Path, bytes: &[u8], mode: u32) -> Result<()>;
    pub async fn list(&self, dir: &Path) -> Result<Vec<DirEntry>>;
    pub async fn exists(&self, path: &Path) -> Result<bool>;
    pub async fn remove_dir_all(&self, dir: &Path) -> Result<()>;
}
```

Wiring, each a small PR with no behaviour change under the current engines:

1. `git::cmd::{git_output, git_output_env, git_output_stdin, run_git, run_git_env}`
   and `git_dist::command(dir)` route through `exec::for_path(dir)`. Under
   `Local` they do exactly what they do now. This single change covers
   `git_state::query`, `content_digest`, checkpoints, disposition, pr_set,
   session_sync, review, capture, attribution, provisioning fixups and the
   publish object reads.
2. `commands/files.rs` (file tree, file read, search) and
   `supervisor/materialize.rs` (writes the brief, skill index, MCP config into
   the checkout) use `Executor::{read,write,list,exists}` instead of `std::fs`
   / `tokio::fs` on checkout paths.
3. `supervisor/shell.rs` (the user's terminal in the checkout) asks the
   registry: under a box it spawns `ssh -F <cfg> -tt box fletch-guest exec --cwd <checkout> -- bash -l`.
4. `supervisor/disposition.rs` and `lifecycle.rs` teardown of a workspace dir
   call `BoxRegistry::discard(agent_id)` when the agent is box-stamped, which
   stops the VM and deletes the box directory, instead of `remove_dir_all` on
   a host tree that holds only a disk image.
5. A test (`exec::tests::no_direct_git_on_checkouts`) greps `crates/fletch-core/src`
   for `Command::new("git")` and `current_dir(` outside `git/cmd.rs`,
   `git_dist.rs` and tests, and fails on a new one.

The Docker and Podman engines are unaffected: their checkouts are host paths
and `for_path` returns `Local`.

### 7.2 `BoxEngine: SandboxEngine`

```rust
// crates/fletch-core/src/sandbox/boxes/mod.rs
pub struct BoxEngine;               // EngineKind::Box
impl SandboxEngine for BoxEngine {
    fn kind(&self) -> EngineKind { EngineKind::Box }
    fn launch_agent(&self, ctx: &AgentLaunchCtx, agent_bin: &str) -> Result<LaunchPlan>;
    fn kill(&self, plan: &KillPlan) -> Result<()>;       // KillPlan::Box { agent_id }
    fn deliver_env(&self, agent_id: &str, launch: &str, env: &[(String,String)]) -> Result<PathBuf>;
}
```

`launch_agent`:

1. `BoxRegistry::ensure_running(agent_id, ctx)`: create the box dir, key and
   `ssh_config` if absent; create or clone the workspace disk (§7.5); build
   the `VmSpec` (base image, data disk, tools disk for `ctx.provider`, memory
   and cpus from settings, env with the three identity paths and the
   authorized key, vsock map: listen 22 → `ssh.sock`, connect 6000 ←
   `egress.sock`, connect 5000 ← `ctl.sock`); remove stale socket paths (spike:
   libkrun fails `EEXIST`); start `EgressProxy` on `egress.sock` with the
   project's policy; spawn `fletch-vm`; read the console until `FLETCH_HOSTKEY`
   (write `known_hosts`) and `FLETCH_READY` (timeout 10 s → fail closed with
   the console tail in the error).
2. Return a `LaunchPlan`:
   - `program`: `/usr/bin/ssh`
   - `prefix_args`: `-F <box>/ssh_config -tt box fletch-guest exec --env-file <guest env path> --cwd <ctx.cwd> -- <agent_bin>`
     where `<agent_bin>` is the CLI's path on the tools disk. The spawn code
     appends the agent's own args, as it does for every engine.
   - `env`: empty for the guest's sake (the ssh process itself needs only
     `PATH`); `env_remove`: the host credential variables as seatbelt lists.
   - `env_delivery: EnvDelivery::GuestFile`, the new field (§7.6).
   - `kill: KillHandle::Engine { plan: KillPlan::Box{..} }`; `kill` sends
     SIGHUP to the ssh process group, which ends the guest PTY session, then
     leaves the VM running for the idle timer.

`AgentLaunchCtx` is unchanged. `ctx.source_repos` are not mounted anywhere:
the box has a full copy (§7.5).

### 7.3 Relays per running box

Started by `BoxRegistry::ensure_running`, stopped with the VM:

- **Mailbox relay**: `ssh -F cfg box fletch-guest relay <rpc_dir>`; host side
  mirrors each request into the host `rpc_dir/requests/`, where the existing
  `supervisor::rpc_watch` tick picks it up unchanged, and watches the host
  `responses/` dir to send responses back. `rpc_dir` on the host is created as
  today; only its contents are mirrored.
- **Transcript mirror**: `ssh -F cfg box fletch-guest tail <home>/.claude/projects`
  (and the provider's equivalent root), appending into
  `<box>/claude-projects/<slug>/…`. The transcript readers already know how to
  read a per-agent projects dir for Docker agents (`projects_src`); the box
  engine registers its mirror dir in the same way so `session_sync` needs no
  new branch. Mirror is append-only and idempotent on reconnect (the guest
  resends from the offset the host reports on connect).

Both relays reconnect with backoff while the VM is running and are torn down
on stop. They are plain `ssh` child processes supervised by the registry.

### 7.4 Git: provisioning, reads, publish

- **Provisioning in the guest** (`boxes::provision`), replacing
  `sandbox::provision::*` for box agents: `ssh box git init <checkout>` then
  host `git push --quiet <box>:<checkout> <base sha>:refs/fletch/base` with
  `GIT_SSH_COMMAND="ssh -F cfg"`, then `git checkout --detach refs/fletch/base`
  in the guest, then the existing fixups (`rewrite_origin`, `seed_identity`,
  `install_delegation_hooks`, attribution hook, `gpgsign=false`) which already
  run through `git::cmd` and `Executor::write`, so they route to the guest
  without change. No `--shared`, no alternates: the box has a full copy of the
  objects. When a template disk exists the clone is already there and this step
  is only `git fetch` of the base from the host plus the checkout.
- **Reads** (`git_state::query`, diffs, status, checkpoints) run in the guest via
  §7.1. No mirror needed while the VM runs.
- **Publish and transport**: `git::remote::Transport` runs in the user's
  repository over the checkout's objects. For a box the host first refreshes
  `<box>/mirror.git` with `git fetch <box>:<checkout> '+refs/heads/*:refs/heads/*' '+HEAD:refs/fletch/head'`
  (the mirror's alternates point at the source repo, so only new objects move;
  spike: 0.17 s), then transport uses `mirror.git/objects` where it used
  `<checkout>/.git/objects`. `boxes::mirror::refresh(agent_id)` is called by
  `git::publish` before publishing, by checkpoints after a checkpoint, and by
  the idle stop before the VM goes down so the diff view has something while
  stopped (`git_state` falls back to `mirror.git` + the last snapshot tree when
  the box is stopped).
- **Hardening**: `git::hardening`'s refusal still runs on the host for
  `mirror.git`; the checkout's own config never reaches host git, which is the
  point of the design.

### 7.5 Disks: workspace, template, tools

```rust
// crates/fletch-box/src/disks.rs
pub fn create_sparse(path: &Path, bytes: u64) -> io::Result<()>;      // truncate
pub fn clone(from: &Path, to: &Path) -> io::Result<()>;               // clonefile(2); falls back to copy with a warning
pub struct Dirty(PathBuf);  // create on start, remove after clean stop; present at start => fsck in init will run; log it
```

- Workspace disk: on first launch, `clone(template, data.img)` if the project
  has a template for the current lockfile hash, else `create_sparse(8 GiB)`.
- Template disk (phase 5b): built by booting a throwaway box on a fresh sparse
  disk, cloning the repo, running the project's setup command through the
  channel, `sync`, clean stop, then renaming to `template-<hash>.img`. Lockfile
  hash = sha256 over the sorted set of lockfiles detected at the repo root
  (`bun.lock`, `package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`,
  `Cargo.lock`, `poetry.lock`, `uv.lock`, `composer.lock`, `Gemfile.lock`,
  `go.sum`).
- Tools disk: built once per `<provider>-<version>` by booting a box on the
  base image with a sparse disk, running the pinned install through the proxy
  (`npm install -g @anthropic-ai/claude-code@<v>` into `/data/tools/claude`,
  the Cursor installer with `HOME=/data/tools/cursor`), `sync`, stop, then
  marking the file read-only. The installed CLI version is what Fletch's
  existing agent version pinning says. A new version is a new disk; old disks
  are pruned when no agent is stamped with them.

### 7.6 Spawn path change: `EnvDelivery`

`LaunchPlan` gains `env_delivery: EnvDelivery { Process, GuestFile }`
(default `Process`). The three spawn shapes in `agent/spawn.rs`
(`spawn_pty`, `spawn_pty_native`, `spawn_managed`, and `spawn_per_turn`)
already assemble the final env as `launch_env + rpc_env + mcp.env`. After
assembling it, when `GuestFile`, they call
`engine.deliver_env(agent_id, launch_id, &env)` which writes the file into the
guest at `/data/run/<launch id>.env` (mode 0600, owner `agent`) over the
channel and returns the guest path that the `--env-file` argument in
`prefix_args` already names (the engine chose the launch id when building the
plan). The local `PtySpawn.env` is then set to the minimum the ssh process
needs. Under every other engine `deliver_env` is a no-op and nothing changes.

Env file contents: the assembled env plus what the guest needs that the host
would not otherwise set: `HOME=<home>`, `PATH=/opt/fletch/tools/<provider>/bin:/usr/local/bin:/usr/bin:/bin`,
`SHELL=/bin/bash`, `TERM`, `HTTPS_PROXY=http://127.0.0.1:8899`, `HTTP_PROXY`,
lowercase twins, `NO_PROXY=127.0.0.1,localhost`, `GIT_TERMINAL_PROMPT=0`,
`FLETCH_RPC_DIR=<rpc_dir>`, `CLAUDE_CODE_OAUTH_TOKEN` (host-resolved, as for
every engine), `CODEX_HOME` when codex. Never: `SSH_AUTH_SOCK`, `GH_TOKEN`,
`GITHUB_TOKEN`, anything from the host login shell.

### 7.7 `BoxRegistry`

```rust
pub struct BoxRegistry { /* agent_id -> BoxState */ }
pub enum BoxPhase { Stopped, Starting, Running { since }, Stopping, Failed { reason } }
impl BoxRegistry {
    pub fn ensure_running(&self, agent_id: &str, ctx: &AgentLaunchCtx) -> Result<Arc<RunningBox>>;
    pub fn channel_for_path(&self, path: &Path) -> Option<Arc<Channel>>;   // §7.1
    pub fn mirror_dir(&self, agent_id: &str) -> Option<PathBuf>;           // transcripts
    pub fn stop(&self, agent_id: &str, reason: StopReason) -> Result<StopOutcome>;
    pub fn discard(&self, agent_id: &str) -> Result<()>;                   // stop + delete box dir
    pub fn attach(&self, agent_id: &str) / detach(..)                       // live process count for the idle timer
    pub fn state(&self, agent_id: &str) -> BoxPhase;
}
```

Stop sequence (`StopOutcome` records which rung worked): `mirror::refresh`;
send `stop` on the control vsock and wait up to 10 s for the helper to exit;
else `ssh box 'sync; echo u > /proc/sysrq-trigger; echo o > /proc/sysrq-trigger'`
and wait 5 s; else SIGTERM the helper, wait 3 s; else SIGKILL and leave
`dirty` in place. The spike showed a killed VM loses recent writes.

Idle timer: when `attach` count is zero for `box_idle_stop_minutes`, stop.
The next `ensure_running` boots again (0.4 s). App exit stops every box
cleanly (hook into `supervisor.shutdown()`).

Startup sweep: on boot, for every box dir with `dirty` present and no live
helper, log it; the next start runs fsck in init. Orphan `fletch-vm` processes
are found by a label in their argv (`--agent <id> --host-pid <pid>`) as the
container sweep does.

### 7.8 Guarantees

`sandbox/guarantees.rs` gains `EngineKind::Box` coverage:
`ConfinedWrites` Enforced; `WithheldGitConfig` Enforced (host git never runs in
the checkout); `OpaqueAppData` Enforced; `UntouchableSourceGit` Enforced;
`ConfinedReads` Enforced; `ConfinedNetwork` Enforced with the note "egress only
through the host proxy's allow-list; see the project's list"; `HostHeldCredentials`
Partial with the shared `brokered_publish_gating!()` text. The compiler forces
this when the variant is added.

## 8. Runtime crate API (`fletch-box`)

```rust
pub struct RuntimePaths { pub helper: PathBuf, pub lib_dir: Option<PathBuf>, pub run_root: PathBuf }
impl RuntimePaths { pub fn bundled(exe: &Path) -> Option<Self>; pub fn from_env() -> Option<Self>; pub fn probe(&self) -> Result<Probe> } // Probe: helper present, signed, dylibs load, hv_support, arch

pub struct VmSpec {
    pub id: String, pub cpus: u8, pub memory_mib: u32,
    pub base_image: PathBuf, pub data_disk: PathBuf, pub tools_disk: Option<PathBuf>,
    pub listen_ports: Vec<(u32, PathBuf)>,    // guest listens; host connects to the unix socket libkrun binds
    pub connect_ports: Vec<(u32, PathBuf)>,   // guest connects; host must already serve the unix socket
    pub env: BTreeMap<String, String>,        // kernel-side env for init (identity paths, authorized key)
    pub console: PathBuf,
}
pub struct Vm { .. }
impl Vm {
    pub fn start(paths: &RuntimePaths, spec: &VmSpec) -> Result<Vm>;   // removes stale listen sockets first
    pub async fn wait_ready(&self, timeout: Duration) -> Result<HostKey>; // parses FLETCH_HOSTKEY / FLETCH_READY from the console
    pub fn pid(&self) -> u32;
    pub async fn stop(&self, ctl: &ControlSocket, grace: Duration) -> StopOutcome;
    pub fn kill(&self);
}

pub struct Channel { pub config: PathBuf /* ssh_config */ }
impl Channel {
    pub fn new(box_dir: &Path, ssh_sock: &Path, key: &Path, known_hosts: &Path) -> Result<Channel>; // writes ssh_config
    pub async fn exec(&self, cwd: Option<&Path>, env_file: Option<&Path>, cmd: &[&str]) -> Result<Output>;
    pub fn pty_command(&self, cwd: &Path, env_file: &Path, cmd: &[&str]) -> (PathBuf, Vec<String>); // program + args for PtySpawn
    pub async fn put(&self, bytes: &[u8], guest_path: &Path, mode: u32) -> Result<()>;   // ssh 'cat > path' with umask; never argv
    pub async fn get(&self, guest_path: &Path, max_bytes: u64) -> Result<Vec<u8>>;
    pub fn git_ssh_command(&self) -> String;                                 // for GIT_SSH_COMMAND
    pub fn forward(&self, host_port: u16, guest_port: u16) -> Result<Forward>; // ssh -f -N -L; Forward kills on drop
    pub fn spawn_relay(&self, args: &[&str]) -> Result<Child>;               // long-lived exec for relay/tail
}

pub struct EgressPolicy { pub allow: Vec<HostPattern>, pub ports: Vec<u16> /* default 80,443 */ }
pub struct EgressProxy { .. }
impl EgressProxy { pub fn start(sock: &Path, policy: EgressPolicy, on_event: impl Fn(EgressEvent)) -> Result<Self>; pub fn update(&self, policy: EgressPolicy); pub fn stop(self); }
pub enum EgressEvent { Allowed { host, port, bytes_up, bytes_down }, Denied { host, port }, Error { .. } }

pub struct ImageStore { .. }  // port of Calina's: PinnedImage::this_build(), current(min), install(progress), verify, prune
```

Default egress allow-list (code constant, documented in `docs/boxes.md`):
`api.anthropic.com`, `platform.claude.com`, `*.anthropic.com`, `*.claude.ai`,
`api.openai.com`, `chatgpt.com`, `auth.openai.com`, `api2.cursor.sh`,
`*.cursor.sh`, `api.cursor.com`, `opencode.ai`, `*.opencode.ai`,
`registry.npmjs.org`, `*.npmjs.org`, `pypi.org`, `files.pythonhosted.org`,
`crates.io`, `static.crates.io`, `index.crates.io`, `github.com`,
`api.github.com`, `objects.githubusercontent.com`, `raw.githubusercontent.com`,
`codeload.github.com`, `deb.debian.org`, `security.debian.org`,
`proxy.golang.org`, `sum.golang.org`, `repo.packagist.org`, `rubygems.org`.
Telemetry hosts (Datadog, Sentry, Statsig) are deliberately absent; the spike
showed the CLIs tolerate their denial. Every denial is an `EgressEvent` the
engine emits as `box:egress` (§9).

## 9. Events and commands (engine ↔ frontend)

Events: `box:state { agent_id, phase, since, reason? }`,
`box:egress { agent_id, host, port, allowed }`, `box:image { phase: not_installed|downloading|verifying|installed|damaged|unavailable, progress?, version? }`.

Commands (Tauri + remote dispatcher, same `*_impl` pattern as every other):
`box_image_status`, `box_image_install`, `box_image_cancel`,
`box_state(agent_id)`, `box_stop(agent_id)`, `box_egress_allow(project_id, pattern)`,
`box_runtime_probe` (helper, dylibs, hypervisor, arch, image: for the Settings
page and for the Developer pane's "Box smoke test" button, which boots a
throwaway box, runs `uname -r` over the channel, stops it, and reports timings).

## 10. Testing

- **Unit, everywhere:** `VmSpec → LaunchConfig`, `ssh_config` generation,
  env-file rendering (no host secrets, the deny-list of names), egress policy
  matching, relay line protocol with a scripted guest on a socketpair, mirror
  append idempotence, `exec::for_path` routing, the `no_direct_git_on_checkouts`
  grep test, guarantees coverage completeness.
- **VM tests, `#[ignore]`d:** boot and ready, hostkey learned, exec, PTY,
  env file isolation (`env` in the guest shows no host variable), git push in
  and fetch out, egress allow and deny and no direct route, port forward,
  template clone and boot, clean stop leaves no `dirty`, kill leaves `dirty`
  and the next boot fscks. `scripts/box-test.sh` runs them with the built
  helper and a local image; CI runs them on a self-hosted Apple-silicon
  runner (hosted runners are VMs without nested virtualization).
- **End-to-end, manual gate for phase 4:** with the flag on, spawn a box agent
  on the Fletch repo, have it edit and commit, see the diff view, open a PR
  through the broker, resume after the idle stop, archive. Repeat with Codex
  and Cursor.

## 11. Phases

Each phase is a PR series; a phase is done when its acceptance holds on the
current engines unchanged and, from phase 4, on a box.

| # | Phase | Scope | Acceptance |
| --- | --- | --- | --- |
| 1 | `fletch-box` runtime crate | `runtime`, `disks`, `image_store`, `egress`, `channel`, `guest.rs`, `bin/fletch-vm`, `bin/fletch-box` CLI; libkrun build scripts and the four patches vendored from Calina under `packaging/libkrun/`; `packaging/box-bundle/` Tauri hooks | unit tests pass on Linux CI; VM tests pass on a Mac against Calina's image (yes, Alpine: the runtime does not care); `fletch-box boot && fletch-box ssh uname -r` works from a dev build; the bundled app contains a signed `fletch-vm` and the dylibs |
| 2 | Guest image and `fletch-guest` | `crates/fletch-guest`, `packaging/box-image/` Debian pipeline, init, sshd config, reproducible build, GitHub release `box-image-v<ver>`, pin in `image_store` | `fletch-box boot` on the Debian image reaches `FLETCH_READY` in under 1 s; `fletch-box ssh` as `agent`; each of claude, codex, cursor-agent, opencode runs `--version` from a tools disk built by `fletch-box tools build <provider>`; relay and tail round-trip in a VM test |
| 3 | Core seams, no behaviour change | `exec.rs` + `git::cmd` routing (3a), `commands/files` and `materialize` on `Executor` (3b), `LaunchPlan::env_delivery` + `deliver_env` no-op (3c), transcript mirror dir generalized from Docker's `projects_src` (3d), `EngineKind::Box` variant with guarantees and settings parsing but `engine_for(Box)` returning `SandboxUnavailable("boxes are not available in this build")` (3e) | existing tests green; a seatbelt, Docker and Podman agent each complete a turn, diff view, publish; the grep test passes |
| 4 | The engine, behind the flag | `sandbox/boxes/`: registry, provisioning, launch, relays, mirror, stop and idle, discard; settings keys; Developer pane toggle; Sandbox page option and image install UI; `box:*` events; isolation panel row; `fletch-host` boot wires the same | the §10 end-to-end gate; flag off → byte-identical Sandbox page and no box code path reachable (test: `engine_for(Box)` refused while the flag is off) |
| 5a | Run in the box | `supervisor/run.rs::sandboxed_run_command` dispatches on the agent's engine: in a box, run through the channel with `-L` to a free host port, `PORT` set, log streamed; verifier and tests gate the same; Run panel shows the host URL | a Vite project's Run opens in the Mac browser; `app_start` from the agent works; a desktop target is flagged "needs a Mac" and not run |
| 5b | Templates and lifecycle (#945) | `run.setup` / `run.teardown` settings, template build and reuse, rebuild on lockfile change, setup log in the run log, failing setup blocks spawn | a Laravel project's second workspace is ready with `vendor/` in under 2 s |
| 5c | Egress UI | denials surface in the workspace with allow once / allow for project / deny; `box.egress_allow` editing in Project Settings | an agent's denied host appears and can be allowed without a restart |
| 6 | Hardening and defaults | agent as `agent` uid everywhere, browser disk split out of the base image, Chromium optional download, memory sampling and the `vm.vfs_cache_pressure` tuning, kernel source offer in About | decided after weeks on phase 4 |

Phases 1 and 2 share no code with 3 and run in parallel. 4 needs all three.
5a, 5b and 5c are independent of each other.

## 12. Nuances the implementer must not rediscover

Each of these cost time in the spike or is a decision already made.

1. **Never pass scripts or quoted commands through libkrun's argv.** Its init
   re-splits a quoted `sh -c`. Commands reach the guest over ssh; init gets
   everything from env or disk.
2. **libkrun binds listen-mode unix sockets itself** and fails with `EEXIST`
   (`-17`) on a stale path. Remove the box's socket paths before every start.
3. **Do not use macOS `nc -U`** in tests or scripts against those sockets; it
   returns nothing after five seconds. Use a real socket client or ssh.
4. **Port forwarding needs no guest relay when done with `ssh -L`**: sshd
   connects to the guest TCP port itself. The spike's `socat` relay was for the
   raw-vsock path only.
5. **A killed VM loses recent writes** (zero-byte files on the next boot). Use
   the stop ladder in §7.7; treat `dirty` as a signal, not an error.
6. **Guest page cache counts against host memory** until the guest releases
   pages; the balloon patch then returns it within seconds. Cap guest RAM, stop
   idle boxes, and report footprint, not guest-reported usage.
7. **Claude Code refuses `--dangerously-skip-permissions` as root.** Run the
   agent as `agent`. If root is unavoidable in a test, `IS_SANDBOX=1`.
8. **An ssh session has no environment.** Everything the CLI needs, including
   the proxy variables, comes from the env file via `fletch-guest exec`.
   Without `HTTPS_PROXY`, Claude Code retries DNS for about four minutes before
   failing.
9. **The image needs `bash`** or Claude Code's Bash tool reports "No suitable
   shell found" and the model refuses to fabricate output.
10. **Seed Claude's onboarding state** (`~/.claude.json` with the theme and the
    accepted notes) into the guest home at first launch, as the Docker
    entrypoint does, or the first interactive session lands in the theme
    chooser.
11. **The access token in the Keychain is short-lived.** Launch resolution and
    refresh stay host-side (`agent::host_login`), unchanged; the box just
    receives the token in the env file.
12. **Cursor needs glibc.** The base image is Debian; do not add an Alpine
    variant. The spike proved `gcompat` is not enough.
13. **Allow the providers' full host sets**, not only the API host: Claude Code
    calls `platform.claude.com` at startup and prints a clear proxy error when
    refused. Denying Datadog, Sentry and Statsig is fine.
14. **`git upload-pack --strict` wants the `.git` directory**, not the work
    tree. Irrelevant with the ssh transport, which is why the design uses it.
15. **The helper is the only thing signed with the hypervisor entitlement.**
    Tauri signs everything in `src/bin` with the app's entitlements, so the
    helper is placed by a `beforeBundleCommand` hook into `Contents/Helpers`
    (Calina's `sandbox/bundle/prepare.sh`). Hardened runtime requires the
    dylibs to be signed by the same team.
16. **The helper must die with the app.** Keep the stdin pipe open from the
    host and have the helper exit on EOF (Calina's `watch_parent`).
17. **Running boxes from inside a Fletch seatbelt sandbox works**, so dogfooding
    Fletch on Fletch keeps working; the ad-hoc signed helper is accepted.
18. **Intel Macs cannot run libkrun's macOS backend.** `RuntimePaths::probe`
    reports `arch` and `engine_for(Box)` returns `SandboxUnavailable` with a
    sentence saying so; the Sandbox page hides the option on Intel.
19. **`libkrunfw` bundles a GPL-2.0 kernel.** The About screen's licence notice
    must offer the kernel source; `NOTICE` lists libkrun (Apache-2.0).
20. **Socket paths are limited to 103 bytes** on macOS. Validate at box
    creation.
21. **Transcript slugs embed the cwd.** Path identity is what keeps
    `~/.claude/projects/-Users-alex--fletch-workspaces-<id>-fletch` the same
    name on both sides; do not rename the guest paths.
22. **The spike's dropbear is not the product server.** OpenSSH `sshd -i` in
    the image gives sftp, `AcceptEnv` control and `AllowUsers`; dropbear
    hardcodes the sftp-server path and has no per-user allow list.

## 13. Implementation notes (phase 4a, as built)

Where `crates/fletch-core/src/sandbox/boxes/` departs from the text above, and
why. The spec stays the design; this is the record.

- **Box dir is `~/.fletch/[dev/]boxes/<agent id>/`**, a sibling of the
  workspaces root, not `<workspace>/box/` (§5). Inside the root it would be a
  host file under the path the seam routes remotely, and socket paths get
  shorter. `sources.json` (checkout → source repo, for the mirror) joins the
  listed contents. The host also keeps an **empty placeholder directory** at
  each box checkout's path: the ssh launcher is spawned with the checkout as
  its cwd, which must exist on the host, and the host-side checks keyed on the
  directory (`hardening::is_inside`) then behave as for any checkout.
- **Routing spans the box's life, not the VM's** (§7.1, §7.7). The writable
  root is registered with `exec::register_root` from the first boot (or boot of
  the app, for live box agents) until `discard`; a routed operation on a
  stopped box boots it again (0.4–1 s) instead of reading a mirror, so
  `git_state` needs no stopped-box fallback. `unregister` happens on discard.
- **No `fletch-guest exec` for host commands.** The executor's `command` is
  `ssh box "cd <cwd> && exec env K=V… <program> <args>"`; `exec --env-file`
  (which clears the environment) is for launches only. Because ssh joins its
  arguments and the guest shell re-splits them, every returned `Command` and
  every `LaunchPlan` is `bash -c <wrapper>` that single-quotes the appended
  arguments and `exec`s ssh (`executor::quoting_wrapper`).
- **Tools disk off the spawn path** (§7.5). A missing disk is built in the
  background (`box:tools` events) and the box boots without one; provisioning
  proceeds, and a *launch* fails closed until the disk exists, then restarts
  the box to attach it. Built once per `<provider>-<host CLI version>`
  (`agent::cached_provider_version`, `latest` without a host CLI). **Blocker
  found while building the first disk**: the install scripts write
  `/data/tools/<provider>`, but init leaves `/data` root-owned and sshd admits
  only `agent`, so `mkdir /data/tools` is refused. Init needs
  `mkdir -p /data/tools && chown agent:agent /data/tools` after the `/data`
  mounts (one line in `packaging/box-image/rootfs/opt/fletch/init`) and an
  image rebuild; the runtime side (`fletch_box::tools::build`, shared by
  `fletch-box tools build` and the engine) is in place.
- **Image pin** (§6.1): `PinnedImage::this_build` carries the values of
  `packaging/box-image/out/manifest.json` (`0.1.0-dev`); `box_image_url` may
  be a `file://` directory holding `base.erofs`, which `ImageStore::install`
  copies and verifies like a download.
- **Providers**: Claude Code only. Codex, Cursor and OpenCode have tools-disk
  scripts, but their logins are files the host writes into a host-side
  overlay; delivering those into a guest is later work
  (`boxes::supports_provider`). Workflow step agents (adopted trees, run
  repos) are refused too.
- **Stop ladder** (§7.7): the sysrq rung needs root in the guest; on the
  product image `agent` cannot write `/proc/sysrq-trigger`, so the rung is
  inert (its wait is skipped) and the ladder goes control → SIGTERM → SIGKILL.
- **Transport and publish** (§7.4): `Transport::open` on a box checkout runs
  the config refusal and the origin check in the guest, then operates over
  the refreshed `mirror.git` (alternates → the source repo). Tracking refs it
  moves in the mirror are pushed back into the guest (`mirror::push_tracking`),
  and a signed copy made on publish is pushed in before the guest branch is
  moved onto it (`mirror::export_commit`). `mirror::refresh` also runs after
  each turn checkpoint and before every non-discard stop.
- **Idle timer**: `attach` is the launch, `detach` the engine's `kill`, which
  every session shape calls once when its process ends (`PtySession::drop`),
  so no second liveness probe is needed. A box booted by a read with no launch
  attached arms the timer at boot.
- **Settings**: five host keys (`boxes_enabled`, `box_memory_mib`, `box_cpus`,
  `box_image_url`, `box_idle_stop_minutes`) written through
  `set_box_setting_impl`, which validates, mirrors in-process and announces
  `settings:changed`; turning the flag off while `box` is the selected engine
  resets the selection to `sandbox-exec`. `box.egress_allow` is written through
  `box_egress_allow_impl`, which also updates running proxies.
- **Native handoff into a box** (`materialize`): the transcript is written to
  the box's `claude-projects` mirror on the host, which the guest does not
  read; a forked conversation is not yet carried into a box.
- **Launch credential**: the container auth chain (`container::auth::resolve`)
  supplies the env — the host-resolved token, else the stored setup-token or
  the shell's own key — so a box signs in exactly like a container.
- **Phase 4b surface** (§4, §9): the Sandbox page shows the image panel and
  the box knobs whenever the flag is on (not only with `box` selected), since
  the option stays disabled until the image is installed; the `box` option
  appears once the probe reports Apple silicon. No "Box smoke test" button:
  `commands/boxes.rs` exposes no smoke-test command yet. Denied egress is a
  dismissable toast, with no allow action until 5c. The remote dispatcher
  answers the eight `box_*` ops and forwards the four `box:*` events;
  `docs/remote-protocol.md` has no rows for them yet.

## 14. Open decisions deferred to implementation

- Rust SSH client (`russh`) versus the system OpenSSH client. This spec uses
  OpenSSH with a generated config, which is what git and editors also use;
  `Channel` is the one place to swap it.
- Whether the tools disk is per provider or one disk with every CLI. Per
  provider keeps a Cursor update from invalidating a Claude box; start there.
- Exact Node provisioning in the Debian image (NodeSource versus distro). Pin
  whichever gives Node 22 LTS reproducibly.
- Idle stop versus suspend. libkrun has no snapshot on macOS; stop and resume
  at 0.4 s is the design.
