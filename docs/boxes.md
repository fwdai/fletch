# Boxes: the next isolation model

Status: design, not yet implemented. `docs/isolation.md` describes what ships
today and stays authoritative for it until the box engine replaces the default.
This document is the contract the box work is built against; deviations are
made here first, then in code.

## Why

Today Fletch has one word, "the sandbox", for three different things fused
together: the isolation mechanism (seatbelt, Docker, Podman), the access policy
(which paths may be written), and the environment a project needs (toolchains,
dependencies, services, env). Because they are fused, every new need widens a
profile: the Run profile grants `~/.local` because builds needed it, reads stay
open because toolchains needed them, network stays open because package
managers needed it. Issue #945 is the same fusion seen from the other side:
there is no declared environment, so agents improvise one inside the box and
collide with the policy.

Seatbelt, the default engine, confines writes only. An agent runs as the user,
reads anything the user can, reaches any network host, inherits the login
shell's environment, and may write caches other host tools execute from. The
container engines are stronger but need a separately installed runtime, run
the Run panel on the host anyway, and double the surface to maintain. Three
engines with three security models is the thing to converge, not extend.

The model below separates the five concerns and converges on one mechanism for
everything that can be isolated, keeping a host runner only for what cannot.

## The five concepts

| Concept | Answers | Lives in |
| --- | --- | --- |
| **Box** | Where a process runs: a micro-VM, a container, or the host under seatbelt | the engine |
| **Access profile** | What a workload may read, write, reach, consume | Fletch settings, never the repo |
| **Environment** | The provisioned computer a workspace needs: checkout, deps, toolchains, services, env, lifecycle | the project environment spec, plus the template disk it produces |
| **Workload** | What runs: the agent, setup, a run target, tests, the verifier | the supervisor |
| **Broker** | The one host door for what a box cannot do: publish, approvals, screenshots of host windows, secrets | `rpc/` |

Rules that follow:

1. A workload runs in a box under an access profile, inside an environment.
2. Access profiles are declarative and engine-independent; every engine compiles
   the same profile to its own mechanism. The guarantees matrix is derived from
   profile and engine, not hand-maintained.
3. The environment spec may live in the repo, because it only says what to run
   inside a box. The access profile never does, because the repo is
   agent-writable.
4. An agent reaches the host only through the broker. Every broker op is
   scoped to the workspace it came from and carries its own approval setting.

## The box

### Host and guest

The host is the machine running the Fletch engine: your Mac, a Mac mini, or a
Linux server running `fletch-host`. The guest is the Linux system inside a
box. The guest is the agent's whole computer: its own kernel, filesystem and
processes. It sees nothing of the host unless the host deliberately exposes it,
and the host exposes no folder.

### Runtime

The primary runtime is a libkrun micro-VM, the approach proven in Calina
(`~/Code/calina/docs/sandbox.md`): Hypervisor.framework on Apple silicon, KVM
on Linux. Measured there: about 120 ms to the first guest command, about 0.4 s
to an agent ready to work, 150 to 255 MB of host memory per idle VM. The
kernel ships inside the app as the `libkrunfw` dylib; the only download is the
guest image. The hypervisor entitlement sits on one small signed helper
process, never on the app.

The guest contract is runtime-agnostic (see "Guest contract"), so the same
guest image can run as an OCI container where KVM is unavailable, which is
most cloud VMs. Apple's Containerization framework is a possible third runtime
behind the same contract; it needs macOS 26 and three downloads (kernel, init
image, rootfs) instead of one, and has no Linux story, so it is not the first
choice.

Seatbelt remains for exactly one job: the host runner for targets that need
the Mac (see "Run targets"). It is no longer an agent engine once the box is
the default. Intel Macs cannot run the VM runtime; they become clients of a
remote host, or stay on the legacy engines until those retire.

### Disks

| Disk | Contents | Sharing |
| --- | --- | --- |
| Base image | Read-only root filesystem: Debian userland, node, python, git, Chromium, build tools, the guest init and SSH server | one file per install, attached read-only by every VM |
| Tools disk | The provider CLIs, installed on first use and updated independently of the base | per host, attached read-only |
| Template disk | One per project per lockfile hash: the checkout cloned, setup run, dependencies installed | source for workspace clones |
| Workspace disk | An APFS copy-on-write clone of the template: home, caches, checkout | one per workspace, exclusive to its VM |

Nothing is rebaked into the base image. The template is a data disk prepared
by booting a VM, cloning the repository from the host over the channel, and
running the project's setup. A new workspace is a clone of that disk file:
milliseconds and no space until it diverges. The template rebuilds when the
lockfile hash changes. Writable disk files are never shared between running
VMs.

The checkout lives on the workspace disk, not on a shared folder: virtio-fs
measured seven to nine times slower than native on small files, which rules it
out for `node_modules` and build trees, and a writable shared folder brings
back the planted-file and symlink class of problems.

### Network

The guest has no network device. Its only exits are host services over vsock,
exposed inside the guest on loopback:

- **Egress proxy.** HTTP CONNECT with a per-workspace allow-list from the
  access profile, names resolved on the host, only public addresses dialled,
  every request logged, denials surfaced in the UI as "agent wants to reach X:
  allow once, allow for this project, deny". Default list: the provider APIs,
  GitHub, the package registries.
- **Workspace channel.** See below.

### Guest contract

The host needs five things from a box: run a command, run a PTY, move files,
speak git, forward a port. That is SSH's feature set, so the guest runs an SSH
server bound to a vsock port, and the host is its only client, through a Rust
SSH library over the socket, with a key the host mints per box. What this
buys:

- no bespoke control protocol to design and harden;
- git transport to the guest is plain git over ssh;
- editors attach through their remote-SSH extensions with the host as a jump,
  which replaces the "cd into the workspace" affordance the disk model takes
  away;
- port forwarding for dev servers is built in;
- the same channel works for a VM on a Mac, a VM on a Linux server, and a
  container, so the host contract never depends on the runtime.

Everything from the guest is untrusted: sizes are capped, paths are validated,
and nothing a guest sends can grant anything.

### How files move

Into the box:

- **The repository.** The guest clones it from the host: the host serves the
  user's repository through git's upload-pack on a channel stream. Once per
  project for the template; each workspace already has the repo and only
  fetches the latest base.
- **Attachments.** Sent in chunks into the workspace; never overwrite.
- **Secrets.** As environment, per workload, from the environment spec. Never
  as files from the user's disk, never the login shell wholesale.

Out of the box, always host-initiated:

- **Commits.** The host runs `git fetch` from the guest over the channel into
  the user's own repository, under per-workspace refs. Publish, review, merge
  and the diff view work from objects the host owns. Host-side git never runs
  inside an agent-writable tree again, which retires the git-config refusal
  and the executable-config carve-outs as a class.
- **Uncommitted state.** `git status` and diffs by exec over the channel, or a
  snapshot tree object the host fetches. Cached at each checkpoint so the UI
  still shows something while the VM is stopped.
- **Files, logs, screenshots.** Fetched by path with a size cap.
- **Transcripts and the RPC mailbox.** A guest-side relay streams the agent's
  transcript lines and mailbox files over the channel; the host keeps its
  current parsers and dispatcher.

### Lifecycle and resources

A box starts when its workspace needs one and stops after idle minutes and at
app exit; Fletch already resumes agents from transcripts, so a restart costs
under half a second. Memory and CPU caps are profile fields. The number of
concurrent VMs is capped; an agent that needs one when all are taken gets the
slot of the one idle longest.

## Access profiles

A profile is a value:

```text
AccessProfile {
  reads:     allow-list of paths, with denies inside      // guest paths
  writes:    allow-list of paths, with denies inside
  network:   allow-list of host:port patterns, or none
  secrets:   named env vars this workload receives
  devices:   screen capture, simulator, camera … (host runner only)
  limits:    memory, cpus, pids, disk, wall clock
}
```

Each engine compiles it: the box to disk layout, proxy policy and VM config;
the seatbelt host runner to SBPL; a container to mounts and a network
namespace. Two built-in profiles, the agent profile and the run profile, are
instances of the same type; a run profile is derived from its target's kind.
Profiles are stored in Fletch's settings, per project, with a global default.
Widening a profile is a user action, remembered per project.

`sandbox/guarantees.rs` keeps its role: one claim per row, coverage declared
per engine. With the box engine, "cannot read the rest of your disk" and
"cannot reach arbitrary network hosts" become enforced for the first time.

## The environment

The environment spec is what issue #945 asks for, generalized. Fletch detects
it and proposes it; the user edits it once per project; every workspace of the
project runs it. It may be committed to the repo (and reads a devcontainer file
when present) because it only decides what runs inside the box.

```text
Environment {
  toolchains:  what the image lacks and the project needs (php, ruby, a pinned node)
  setup:       commands run once into the template            // run.setup
  teardown:    commands run when a workspace is archived      // run.teardown
  services:    databases and the like, per workspace
  env:         which variables each workload receives
  targets:     see below
}
```

Lifecycle, each step logged to the run log and failing loudly:

1. **Template build**, once per project per lockfile hash: boot, clone,
   install toolchains, run setup.
2. **Workspace create**: clone the template disk, fetch the latest base,
   admit the agent. A workspace whose template failed does not admit anyone;
   the agent never has to discover a broken tree one tool call at a time.
3. **Archive**: run teardown, release the disk.

Toolchains the image does not carry are the honest cost of a sealed box:
seatbelt borrowed them from the host for free, and that is precisely the
property that made it leaky. They are installed once per project into the
template, never per agent.

## Workloads and run targets

A target is a repo, a working directory, commands, a ready signal, and a
**platform**:

| Platform | Examples | Where it can run |
| --- | --- | --- |
| Portable | web app, API, CLI, tests, a Linux build of a desktop app | the box, on any host |
| Host-only | macOS desktop app, iOS simulator, Android emulator, anything needing the Mac's GUI or devices | a Mac host, outside the box |

Rules:

- **Portable targets run in the box**, in the workspace's own environment,
  with the port forwarded to the host and on to the client's browser. The
  agent screenshots them with the browser inside the box. The verifier and the
  tests gate run the same way.
- **Host-only targets are flagged, not run.** The panel says "this target
  needs a Mac" with the reason. When the Fletch host is the user's own Mac, the
  panel offers to run it through the **host runner**: a seatbelt-confined run,
  under a profile derived from the target kind (app state paths, Xcode roots),
  on a **mirror checkout** the host fetches from the box and installs
  dependencies into on macOS. This is a user action; the agent's `app_start`
  only ever runs targets in the box. On a Linux host nothing runs, and the
  panel names which host could.
- **Running project code unconfined on the host is not a mode.** Unmerged code
  is unreviewed code, whoever triggered it.
- **Multi-repo and multi-target** fall out of the shape: a workspace with a
  backend and a frontend has two targets, each with its own port.

## Multi-host placement

Three tiers, two channels:

| Tier | Channel |
| --- | --- |
| Client (desktop, phone) | control plane to one or more hosts: the existing Noise-over-WebSocket protocol, relay, pairing, push |
| Host (the engine, repositories, credentials) | workspace plane to each box it owns: SSH over vsock or a unix socket |
| Box | speaks only to its host |

Boxes always live on the host that owns the workspace, next to the repository
it fetches from and publishes with. The client never talks to a box directly;
everything it sees comes through the host, including dev-server previews,
which the host forwards to the client over the control plane's existing
stream multiplexing. SSH between client and host is not the control plane; it
is a bootstrap and bastion tool, the natural way to install `fletch-host` on a
server and read back the pairing code.

A target's platform decides which host can run it, across machines as within
one.

## Coexistence with the current engines

The box is a fourth `EngineKind`, behind the existing setting, stamped per
agent as today, declared in the guarantees matrix, off by default until it has
been lived on. The three current engines keep working untouched.

The seam is not the `SandboxEngine` trait. Launching fits it: as Docker returns
a local bridge process attached to a container, the box engine returns one
attached to the guest PTY. What does not fit is every place the host reads a
workspace by its path on disk, about 43 call sites across 20 files, plus the
transcript readers and the file-based RPC mailbox. The seam is a `Checkout`
access abstraction with two implementations: local filesystem, which is
exactly today's behaviour for the three current engines, and remote, over the
workspace channel. The call sites migrate to the abstraction as a pure
refactor first, under the existing engines, with the existing tests. The box
engine then adds the second implementation.

Retiring an engine later is deleting its implementation, not unpicking the
host. Flipping the default is one setting.

## What the box does not claim

- Hypervisor escapes are out of scope; the design relies on
  Hypervisor.framework and KVM.
- A host-only target on the host runner has seatbelt's guarantees, not the
  box's, and the panel says so.
- A container runtime where KVM is unavailable has container guarantees. The
  guarantees matrix states the difference per runtime.
- The user cannot open a workspace in Finder. The replacement is remote editor
  access through the host.
- `libkrunfw` bundles a GPL kernel, so the app must offer its source.

## Open questions, settled by the spike

The spike runs on a branch and ends in a report against these criteria:

1. A Fletch checkout on a guest workspace disk; `claude`, `codex`,
   `cursor-agent` and `opencode` each complete a turn in a PTY over the
   channel on a Debian-based image, and `--resume` works.
2. The host fetches the guest's commits over the channel into the user's
   repository and the diff view renders from them.
3. A Vite dev server in the guest is reachable from the Mac's browser through
   the host forward; the first-connection stall reported against libkrun's
   listen-mode vsock mapping is measured and worked around if present.
4. A second workspace is cloned from a template disk and is ready in under a
   second with dependencies present.
5. Memory per idle and per active VM, boot and ready times, and template build
   time are recorded.
6. KVM availability on the server classes we care about is checked, and the
   container fallback boots the same guest image.

## Spike findings, phase 1 (2026-10-28)

Run from inside a Fletch seatbelt sandbox on an M-series Mac, macOS 27, using
Calina's built artifacts unchanged: libkrun 1.19.6 with its four patches,
libkrunfw 5.6.2 (Linux 6.12), and the 496 MB Alpine 3.24 image (node 24,
python 3.14, git 2.54, socat, Chromium). The throwaway runner, guest scripts
and host helpers are in `spike/boxes/` with a reproduction recipe. Guest
memory was 2 GiB, 2 vCPUs. Hosted-CI-style wall clocks, single runs.

| Criterion | Result | Measured |
| --- | --- | --- |
| 1. Provider CLIs run in the guest | **Partly met.** Claude Code 2.1.312, Codex 0.165.0 and OpenCode 1.19.4 all install and start on the musl image. Cursor not testable: the image has no `bash` for its installer and its binary wants glibc. No real turn was run: this sandbox holds no provider credential. | all three installed through the proxy in 23 s, 1.1 GB on disk |
| 2. Host fetches the guest's commits | **Met.** A commit made inside the guest was fetched over vsock into a host repository that borrows the user repo's objects; only the new objects moved. | 0.17 s, 3 objects |
| 3. Dev-server port reaches the Mac's browser | **Met.** `curl` on the host hit a server inside the guest through a TCP bridge, libkrun's listen-mode vsock socket and a guest `socat` relay. The reported first-connection stall did not reproduce with a proper client. | 2.2 ms first request, 1.4 ms after |
| 4. Second workspace from a template disk | **Met.** APFS clone of the prepared data disk, then boot with repo and CLIs present. | clone 27 ms for 1.8 GB used; boot, probe and shutdown 0.5 s |
| 5. Timings and memory | **Recorded.** First boot including formatting the data disk 0.9 s; later boots to a full probe 0.36 s; service guest ready 0.38 s. Host footprint 129 MB idle; 1776 MB after the npm installs, which is the guest's page cache; 465 MB ten seconds after the guest dropped its cache, so Calina's balloon patch does return memory. | see left |
| 6. KVM on servers, container fallback | **Not run.** Needs a Linux machine. | |

Also measured: cloning the whole Fletch repository into the guest over vsock,
30,069 objects and 30.9 MiB, took 1.00 s. The egress proxy allowed
`registry.npmjs.org` (HTTP 200 in 0.10 s), refused `example.com` with 403,
and with it bypassed the guest had no route and no DNS at all. Claude Code at
startup reached for `platform.claude.com`, was refused, and printed a clear
"a proxy is configured, check that it allows this host" message: the default
allow-list must carry the providers' full host sets, not only their API hosts.

What the spike taught that the design did not say:

- **Scripts never travel in argv.** libkrun's init re-splits a quoted `sh -c`
  line, as Calina also found. Attaching a 512-byte-padded script as a raw disk
  and running `/bin/sh /dev/vdc` sidesteps it. The real guest init receives
  its work over the channel, so this only affects the spike.
- **libkrun binds listen-mode sockets itself** and fails with `EEXIST` on a
  stale path. The runtime must remove a workspace's socket paths before each
  launch.
- **macOS `nc -U` is unusable against those sockets**: it returns nothing
  after five seconds. The initial "all connections stall" scare was that, not
  libkrun. A plain socket client round-trips in a millisecond.
- **Port forwarding needs a guest-side relay.** libkrun forwards a host
  connection to a guest *vsock* port; something in the guest must relay it to
  the TCP port the dev server listens on. That relay is one of the guest
  agent's jobs, alongside the SSH server.
- **Guest page cache counts against the host** until the guest frees it.
  Memory per box should be read as "the guest's RAM cap" in the worst case,
  with reclaim catching up within seconds once the guest releases pages.
  Capping guest RAM at 2 GiB per box and stopping idle boxes stays the right
  default; `vm.vfs_cache_pressure` or a periodic cache drop in the guest is
  worth a look.
- **Calina's image lacks `npm` and `bash`.** Bootstrapping npm from its
  registry tarball took under a second, so a Fletch image simply includes
  both; the base-image question (Alpine versus Debian) stays open only for
  Cursor and any other glibc-only binary.
- **Running from inside a seatbelt sandbox works.** Hypervisor.framework
  accepted the ad-hoc-signed helper, so a nested Fletch developing Fletch can
  boot boxes.

Still open after phase 1: a real agent turn over a PTY with a credential, the
Debian versus Alpine base image for Cursor, the guest SSH server in place of
the spike's `socat` channels, and the KVM and container-fallback check on a
Linux host.

## Delivery

Each slice is a PR on its own; 2 and 3 share no code with 4 and may run in
parallel; 5 waits for all three.

| # | Slice | Done when |
| --- | --- | --- |
| 0 | Spike on a branch | the six criteria above, with numbers |
| 1 | This document | agreed |
| 2 | `fletch-box` runtime crate: libkrun build and patches, signed helper and bundling, image store, VM lifecycle, data disks, egress proxy, ported from Calina | unit tests pass everywhere; VM tests pass on a Mac runner |
| 3 | Guest image pipeline: Debian base, init, SSH on vsock, toolchains, tools disk, reproducible build, pinned release | each provider CLI runs a turn inside it |
| 4 | `Checkout` abstraction with the local implementation: the pure refactor of the path-reading call sites, the mailbox and the transcript readers, split by area | no behaviour change; existing tests green on all three engines |
| 5 | The box engine: launch bridge, template and workspace disks, remote `Checkout`, relays, Run forward for portable targets, guarantees row | an agent completes a task, commits, publishes a PR; diff view, resume and the Run panel work |
| 6 | Environment spec and lifecycle (#945): setup, teardown, services, per-workload env, template rebuild on lockfile change | a Laravel project's workspace is ready with `vendor/` and a migrated database, and fails loudly when setup fails |
| 7 | Run targets with platform, access profiles as values, the host runner on a mirror checkout | a Tauri project's web target runs in the box and its desktop target is flagged and runnable from the host runner on a Mac |
| 8 | Multi-host preview forward over the relay | a browser on the client reaches a dev server in a box on a remote host |
