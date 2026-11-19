# Cloud execution: the runner and provider sandboxes

Status: design agreed 2026-11-19. The sandbox half is merged and ran on Fly.io
that day (section 5). The six steps of section 6 are built and in review
(#1149 to #1154), with tests against a stand-in provider only: none has run on
a real machine. Every statement
here is a decision Alex made, a fact checked in the code at the path given,
measured, or quoted from a provider's documentation, or it is marked **open**.
Nothing in section 8 may be assumed.

## 1. The goal

In Alex's words:

- Launch a cloud agent from any device that can spawn agents: a laptop with
  Fletch, or the phone.
- The agent runs autonomously in the cloud, with no device connected.
- Any app on any device that is online and connects sees the agent running and
  its transcript.
- When the agent has finished, the user reviews the result from any device:
  the transcript with its messages and tool calls, and the file changes.
- The user opens the pull request from a client. The agent does not need to
  open it on its own.
- A new agent starts quickly: the user does not wait for a machine to be
  created.

## 2. Two separate choices

"Cloud" was used for two different things. They are independent:

- **Where the supervisor runs**: the Fletch software that owns agents, their
  transcripts and their lifecycle. On the user's device, or on an always-on
  machine.
- **Where the sandbox comes from**: a VM on the supervisor's own machine, or a
  machine at a provider.

| Supervisor | Sandbox | State |
| --- | --- | --- |
| The user's device | Local VM, or the macOS sandbox | Shipped |
| The user's device | Provider machine | Merged (#1135, #1136, #1137, #1138, #1143, #1144, #1146); ran on Fly.io 2026-11-19 |
| A paired host (`fletch-host`) | Local VM on that host | Shipped; needs KVM on Linux |
| A **runner**: `fletch-host` on a provider machine | Provider machine | To build (section 6) |

Only the last two rows meet the goal: with the supervisor on the user's device,
a closed laptop ends the turn, and a phone cannot launch anything. The last row
is the one this document specifies. The second row stays: cloud compute driven
from a laptop, useful and not autonomous.

## 3. Business decisions

1. **The free app sets everything up in one guided setup.** In Settings →
   Cloud the user pastes a Claude token, presses Create and confirms a GitHub
   code; nothing is typed on the runner. Running agents unattended in the
   cloud, on the user's own provider account, is free. Nothing in the app is
   gated. (First worded "in one click"; reworded 2026-11-19, since neither
   credential can be had without a sign-in in a browser.)
2. **The paid product is a Fletch cloud account**, for solo developers and
   teams: conveniences worth paying for. Doing the same things by hand
   (pairing each device, adding projects on each, signing in on each) stays
   possible and free.
3. **No Fletch backend is built now.** No Fletch-operated always-on server and
   no database. Alex is the first user; the solution must cost him only his own
   provider bill.
4. **What the paid account is for** (not designed, not started): a registry of
   devices and hosts so they appear everywhere without pairing codes;
   credentials, projects, sandbox settings and grants managed per user and
   device; one record of what was launched, by whom, with which credentials
   and sandboxes.
5. **A provider is an adapter.** A generalized core behaves the same whatever
   the provider; an adapter holds everything specific to one. Adding a provider
   is adding an adapter, with no other wiring. Fly.io first, Daytona second.
6. **Tools are auto-approved for every provider.** Agents are sandboxed so they
   can run with full autonomy and be reviewed at the end. No design may need a
   person or a component to approve tool calls.

## 4. The design

**A runner per user.** The runner is `fletch-host`, the existing headless host,
on a machine at the user's provider. It supervises: it owns sessions and
transcripts, holds the logins, and serves every paired device through the
relay. It does not run agents on itself.

**A provider machine per workspace.** The runner sandboxes each workspace
exactly as a laptop can since #1143: a box whose machine is at the provider,
at rest when idle, destroyed with the workspace. The code is in `fletch-core`,
which `fletch-host` runs. Archiving a workspace winds its machine down; the
others are untouched.

**Fletch's image, not the provider's.** Every machine runs the box image, the
one built for VMs on the user's machine, with its tools already in it. For a
provider it is the cloud image (`packaging/cloud-image`), built from the box
image's x86_64 release. The runner's machine runs that same image: `fletch-host`
is installed on its durable disk and started by the image's entrypoint when
the disk holds its start script, so `fletch-host update` updates it in place
(decided 2026-11-19; a separate runner image would be fixed when its machine
is created).

**A spare is always ready.** The supervisor keeps a machine created ahead of
any workspace and parked, so a new agent starts on a machine that only has to
wake:

- A spare is a whole box with no owner yet: the image, its disk, its ssh key,
  its host key pinned, and on its disk every agent CLI the supervisor has a
  login for, at the version it would launch, so that a launch on a fresh
  spare creates no machine and installs nothing.
- A new workspace takes the spare. Its replacement is created at once, in the
  background.
- A supervisor that keeps spares always has at least one, whatever the number
  of workspaces, including none. How many is a setting, per connected
  provider; its default depends on the supervisor (last item).
- With no spare ready (two agents spawned in a row, a spare that failed), the
  workspace's machine is created directly, as it is today. A spawn never fails
  or waits for want of a spare.
- A spare made for another image, size or agent CLI version than the
  supervisor now uses is stale, and the next replenish destroys and replaces
  it (at startup, when a provider is connected, with every new cloud box, and
  when the setting changes). Until then a new workspace may still take a stale
  spare whose machine fits (the provider, the image and the size): it skips
  the machine's creation, and its launch brings the CLI to the version it
  needs, which costs one install. A take never asks which CLIs the supervisor
  is signed in to, so it waits on no login probe.
- The runner keeps one spare by default. A laptop that drives provider
  machines itself keeps none by default, since a spare is billed whether or
  not it is used; the same setting turns it on.

**A machine serves one workspace in its life.** A spare has never had an
owner: it is created new, and holds only the image and the agent CLIs its
preparation installed. Once a workspace takes it, it is that workspace's
until the workspace is archived, and then it is destroyed, never parked again
or returned to the spares. So an agent never starts on a machine another
workspace used, and there is nothing of an earlier agent's to clean: a new
workspace on a spare finds its checkout directory and its home as a machine
created for it would have them. Preparing a spare leaves nothing in either.

**Parked means the fastest way back.** A spare, and a workspace's machine
while the workspace is idle, rests in whichever state its provider wakes
soonest from. On Fly.io that is suspended, where its limits allow (section 5),
else stopped. Which state is the adapter's answer, not the core's. Parking
keeps what is on the machine: a workspace wakes to its own files as it left
them. Being pristine comes from never reusing a machine, not from parking.

**A spare gets its identity when it is taken, over ssh.** A box binds the
paths of its workspace, which carry the workspace's id. A spare is created
before that workspace exists, and Fly.io's documentation does not say that
changing a machine's configuration keeps a suspended machine's snapshot
(an update takes the machine down and brings it back up), so a spare is never
given its workspace through the provider. The guest takes its workspace's
paths once, from its supervisor, after it wakes, and the spare's directory on
the supervisor becomes the workspace's box directory.

**Devices are clients.** A laptop or phone paired with the runner launches,
watches and reviews through the existing remote protocol. Nothing about the
session lives on the device.

**Credentials.** The runner holds a GitHub token and a model login of its own,
never a copy of the laptop's login. For the first version (decided
2026-11-19):

- GitHub: the runner runs its own device login while it is set up, and the app
  shows the code. The token is minted on the runner and never leaves it; a
  sandbox never gets it (pushes and pull requests are the supervisor's).
- Claude: a long-lived token the user mints on the laptop with
  `claude setup-token`, written to an owner-only file on the runner's disk and
  loaded into `fletch-host`'s environment. Other agent CLIs are not set up on a
  runner yet.

What reaches a sandbox is not a boundary this design adds: it is what any box
gets from its supervisor, which differs by agent CLI
(`crates/fletch-core/src/sandbox/provider/`):

- Claude, where the supervisor has a login of its own: an access token of
  about 8 hours per launch, the refresh token kept on the supervisor. Where it
  has only a token in its environment, which is the runner's case, that token
  is forwarded unchanged (`auth.rs`): **each sandbox of a runner holds the
  long-lived Claude token.**
- Codex: a login file with the refresh token removed, and the supervisor's
  `OPENAI_API_KEY` if it has one.
- Cursor: its API key.
- opencode and pi: the CLI's own login file, copied into the sandbox and
  reconciled back, and any provider API keys the supervisor has.

So only Claude with a supervisor-side login, and Codex's file, are short-lived
in a sandbox; API keys and the opencode and pi login files are as long-lived
there as on the supervisor. Keeping a runner's sandboxes on short-lived Claude
tokens needs a login on the runner itself (its own refresh token); not built.

**Always on.** In the first version the runner runs until the user stops it
from Settings: a stopped runner cannot be reached from a phone.

**The runner's disk.** Everything the runner owns is in `fletch-host`'s data
directory: the database with the workspaces and sessions, the transcripts, the
logins, the provider connection, the host key that is its identity, and the
paired devices. That directory is on the provider's durable disk, never on the
machine's own root filesystem, which a provider may reset at a stop. The
adapter already has this split for a box: a disk created with the machine and
mounted at the guest's `/data`, which outlives the machine
(`crates/fletch-core/src/cloud/mod.rs`); the runner is created the same way,
with `--data-dir` on that mount. So:

- A stop and a start keep everything.
- A runner whose machine is lost gets a new machine on the same disk
  (`Client::replace_machine`) and is the same runner: same identity, same
  pairings.
- A runner whose disk is lost is lost: its history, logins and pairings with
  it. It is reported as such and never recreated on an empty disk behind the
  user's back; the user creates a new runner and pairs again.
- Deleting the runner deletes its disk, and says so first.

**Later, the paid control plane registers runners; it does not replace them.**
Execution stays in a runner per user or team, so one tenant's credentials are
never in a process shared with another's. The central record is the control
plane, fed by runners.

### Rejected, with the reason

- **Running agents directly on the runner, with the machine as the sandbox.**
  Not needed once the runner uses provider machines, and it would break the
  rule that Fletch has exactly two isolation engines (`docs/isolation.md:26`).
- **One runner per workspace.** Every device pairs with every host one by one
  (section 5), so each workspace would need a pairing on each device.
- **One central supervisor for all users.** It would hold every user's GitHub
  token and model login in one process, and is fixed infrastructure paid for
  before anyone pays.
- **A component in the sandbox that approves tools, logs output and replays it
  to a reconnecting laptop.** It solved problems the laptop-as-supervisor
  design created; the host already keeps a turn running and lets clients catch
  up (section 5).
- **Gating one-click setup or unattended runs behind payment.** Decision 3.1;
  it would also need the accounts and licensing backend of decision 3.3.
- **Deleting provider-side boxes this install has no directory for.** Two
  installs (a debug and a release build, two machines) can share one provider
  account and would destroy each other's workspaces.

## 5. What exists (checked in the code)

The host:

- Agents belong to the host's supervisor, not to a connection. A disconnect
  cancels only that connection's in-flight requests
  (`crates/fletch-core/src/remote/server.rs`).
- A client that connects mid-turn reads the turn so far with `read_live_turn`,
  the host's in-memory copy capped at 2000 events
  (`crates/fletch-core/src/supervisor/live_turn.rs:33`); finished turns come
  from the session records. `hello` returns the whole workspace snapshot.
- On the wire: `spawn_agent`, `read_live_turn`, the session and diff reads,
  `push_agent`, `create_pr`, `clone_repo`, `ensure_project`
  (`crates/fletch-core/src/remote/dispatch.rs`). Pushes and pull
  requests use the host's GitHub login, never the client's.
- A repository must be on the host's disk. The host clones it from GitHub with
  its own token; a phone can ask for that (`clone_repo`). No client transfers
  a repository.
- Every tool call is auto-approved; only `AskUserQuestion` and `ExitPlanMode`
  are held for a person (`crates/fletch-core/src/managed_session.rs:39`).
- The GitHub token comes from `fletch-host github login`, the
  `FLETCH_GITHUB_TOKEN` variable, or a systemd credential
  (`crates/fletch-core/src/host/boot.rs:475,918`). A headless host keeps
  secrets in its settings table, in plaintext (`boot.rs:330`).
- Release builds include `fletch-host` for `x86_64-unknown-linux-gnu`
  (`.github/workflows/release.yml:349`).

Pairing and the relay:

- A host's identity is its public key. A device reaches it through the relay
  only when it holds that key and the relay's address.
- Pairing is one device to one host, with a code minted on the host
  (`fletch-host pair`, over the host's admin socket). There is no account, no
  device or host registry, and no non-interactive pairing. The relay "holds no
  keys and no accounts" (`relay/README.md:197`).
- A paired device is a record in the host's `remote/devices.json` holding the
  device's public key, which the host checks at every handshake
  (`crates/fletch-core/src/remote/auth.rs`).
- A `fletch-host` has no relay unless started with `--relay`: the default
  (`wss://relay.fletch.sh`, `crates/fletch-core/src/remote/mod.rs:74`) is
  written only by the desktop (`boot.rs:239`).

The sandbox half:

- A workspace's box is at a provider when `workspaces.cloud_provider` names
  one; its engine stays `box`. The provider-side box is named by a handle kept
  in the box directory (`crates/fletch-core/src/sandbox/boxes/cloud.rs`).
- Nothing outside `crates/fletch-core/src/cloud/` names a provider, and nothing
  under `src/` does; a test on each side fails otherwise
  (`cloud/tests/neutral.rs`, `src/providerNeutrality.test.ts`). Settings builds
  its connect form from what the adapter describes.
- The cloud image is `ghcr.io/fwdai/fletch-cloud:<box version>-<fingerprint>`,
  pinned in `crates/fletch-box/src/image_store/mod.rs` and checked by
  `scripts/check-cloud-image.sh`.

Measured on Fly.io on 2026-11-19, from the app's log (one run each):

- A new workspace took about a minute to its agent starting. After Fly.io
  reported the machine created: 0.9 s to read the host key and connect over
  ssh, 19 s installing the agent CLI, 10 s writing its version marker and
  syncing the disk, 8 s on the CLI capability probe and launch preparation.
  The creation before that was not timed.
- Waking the same workspace from stopped took about 4.4 s to a connected box
  and about 15 s to the agent's process. The other 11 s was the engine's: a
  turn checkpoint that ran out its 10 s timeout while the box started, and a
  delivery that failed once and was queued again.
- A local VM the same morning was ready in 1.8 s.
- The machine has no `/dev/kvm` and its CPU shows no virtualization flags: a
  Fly.io machine cannot boot a VM. Its image reads `x86_64`.

What Fly.io documents (`docs.fly.io/reference/suspend-resume`,
`machines/api/machines-resource`, `machines/overview`, `about/pricing`):

- Creating a machine "can take some time, maybe low double digit seconds".
- A suspended machine resumes in "a few hundred ms"; a cold start is "~2+
  seconds for common apps". Suspend is `POST …/machines/{id}/suspend`, and
  the ordinary start resumes it.
- A machine can suspend with at most 2 GB of memory, no swap and no schedule.
  A box has 2048 MiB by default (`sandbox/boxes/settings.rs`).
- A resume is not guaranteed: a machine cold-starts when its snapshot is lost,
  its host is maintained, it is migrated, or it is stopped while suspended.
- "Unlike stop, suspend does not reset the machine's rootfs". A stop resets
  it unless the machine asks otherwise; a box asks for it to be kept across a
  restart (`rootfs.persist`, `crates/fletch-core/src/cloud/fly.rs`), and its
  workspace is on its disk either way.
- After a resume the clock can lag until NTP syncs, and connections the
  machine held may be dead.
- A stopped or suspended machine is billed for its root filesystem only,
  $0.15 per GB a month; a volume is $0.15 per GB a month in any state.
- A machine can be created without being started (`skip_launch`).

What stopped a runner on `main` at 2026-11-19, and the step of section 6 that
removes it:

- Off macOS a new agent was always a local box, and a host that cannot boot
  one refused every spawn. Step 4: such a host gives a spawn that names no
  workspace type a box at a connected provider.
- A remote spawn carries no workspace type (`remote/dispatch.rs`), so a client
  cannot ask a host for a provider machine. Still true, and not needed: the
  host's default covers a runner.
- Connecting a provider was a desktop command only. Step 4: it is also on the
  host's local admin socket (`fletch-host cloud connect`); it stays off the
  remote wire.
- No image started `fletch-host`. Step 5: the cloud image's entrypoint starts
  a host service its disk holds.

## 6. What to build

In this order; each is one pull request. Steps 1 to 3 make a new agent start
quickly and are tested from a laptop; steps 4 to 6 make it run unattended.
Built 2026-11-19 as #1151 (step 1), #1149 (2), #1152 (3), #1150 (4), #1153 (5)
and #1154 (6); every "Done when" below is still to be checked on a real
machine.

1. **The engine's own delays.** No disk sync that a launch waits on; the CLI
   capability probe remembered per CLI version, not per box; a wake that does
   not run out the turn checkpoint's timeout or fail its first delivery. Done
   when the log of a wake shows the agent's process starting within a second
   or two of the box being connected.
2. **Parking.** The adapter can put a box at rest in its fastest-waking state
   and wake it, falling back to a stop where that state is not available, and
   an idle workspace's machine is parked that way. Done when a parked Fly.io
   box resumes with its files as they were and is connected again, and a box
   over the suspend limit still stops and starts.
3. **Spares.** The supervisor prepares a box with no owner (created, agent
   CLIs installed, parked), hands it to the next new workspace with that
   workspace's identity given over ssh, and replaces it; with none ready it
   creates the machine directly. The count is a setting. Done when, with a
   fresh spare ready, a new workspace's agent starts without creating a
   machine or installing a CLI, in a checkout directory and home that hold nothing but
   what its own launch put there; a second spare exists shortly after; and
   archiving the workspace destroys its machine and returns nothing to the
   spares.
4. **A headless host sandboxes in provider machines.** It can hold a provider
   connection, and where it cannot boot a local VM it gives a new workspace a
   provider machine instead of refusing. Done when a `fletch-host` on a Linux
   machine without VM support, given a connection, runs a workspace in a
   provider machine.
5. **The runner's machine and its hands-off start.** A machine from the cloud
   image, with `fletch-host` on its disk, that comes up with the relay on, the
   creating device authorised, and its GitHub token and provider connection in
   place, with no terminal, and its data directory on the durable disk. The
   creating device sets it up over the ssh channel the adapter gives, through
   the host's admin socket; nothing secret is in the machine's configuration
   or on the remote wire. Done when a machine set up that way is reachable
   from the creating device through the relay, and after a stop and a start it
   is the same runner: same identity, pairings, logins and history.
6. **One guided setup in the app.** Settings → Cloud creates, shows, stops,
   starts and deletes the runner through the provider adapter. Creating it
   pairs this device, and the runner appears as a host to run on; another
   device pairs with the QR code or link the pane shows (a typed code carries
   no relay address, so it works only on a local network). The runner keeps
   the spares. Done when, from a laptop with a connected provider: paste a
   Claude token, press Create, confirm the GitHub code; then launch a
   workspace on the runner and see its agent start on a spare, close the
   laptop, follow it from the phone.

## 7. Rules for whoever builds this

- No provider is named or branched on outside its adapter, in Rust or in
  `src/`. Keep both tests passing. What differs per provider is asked of the
  adapter.
- No approval machinery (decision 3.6).
- Two isolation engines; no third.
- A box whose disk is gone is refused, never recreated on an empty disk. The
  same holds for the runner.
- A spare is the supervisor's, known by its own directory: the sweep that
  destroys boxes no workspace owns never takes one, and deleting the runner
  destroys its spares with it.
- A spare is never given its workspace by changing its machine's
  configuration.
- A machine is never handed from one workspace to another.
- A guest must work after a resume and after a cold start alike: nothing may
  depend on which one it got.
- A provider token and every other secret stay out of logs, errors and
  telemetry.
- Local boxes, the macOS sandbox and the laptop-driven cloud box behave as
  they do today.
- Before designing around an obstacle, establish that it is a requirement of
  the goal in section 1 and not an artefact of an earlier design.

## 8. Open

Settled while building (2026-11-19):

- Credentials for the first version: section 4, "Credentials".
- The creating device learns the runner's public key, has its own key
  authorised, and hands over the provider connection, all over ssh through the
  host's admin socket (`fletch-host call`), with the host key pinned through
  the provider's authenticated exec as for any cloud box.
- The guest takes its workspace's paths from its supervisor at every boot:
  the supervisor binds them over ssh, a no-op where the entrypoint already
  bound them from the machine's environment. No image or configuration change.

Not investigated:

- A login on the runner itself for Claude (its own refresh token), so that
  its sandboxes get short-lived tokens; and how any other agent CLI is signed
  in on a runner.
- Whether a provider restarts a machine it was asked to keep running after a
  host restart, and what an explicit stop does under a restart policy:
  Fly.io's documentation says neither.

Not verified:

- How long a new agent takes on a spare, end to end. Fly.io's figure covers
  the resume alone; connecting, giving the box its workspace and pushing the
  checkout into it come on top.
- What a spare costs: its root filesystem's size has not been read.
- Whether a suspended machine keeps its snapshot across a configuration
  update. The design does not depend on it.
- How long the creation part of a new machine takes on Fly.io.
- The runner's size and cost.
- `fletch-host` on Linux end to end: the acceptance line in
  `docs/multi-host-plan.md` (3e) is unchecked, and that document still
  describes container engines the code has retired.
- Whether the later control plane fits Cloudflare's free tier.

Known gaps in the sandbox half, accepted for now:

- A crash during the create call leaves a box at the provider that Fletch has
  no record of.
- A machine keeps the image it was created with.
- A changed agent CLI version is installed in place at the next launch, even
  while another session runs in the box.
- A provider box has open network (no allow-list), mounts no folder from the
  user's machine, has no codegraph, and cannot be moved to or from a local
  sandbox.
