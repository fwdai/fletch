# Isolation

Isolation is the feature Fletch is built around: you can run many agents on real
code and trust that none of them can damage your repository, your machine, or
each other's work. This document is the canonical description of how that
isolation works. It is the source of truth — the README summarizes it, the
website should track it, and the code implements it. Where a claim is
load-bearing, the authoritative file is named.

## The model in one paragraph

Every agent works in its own **clone** of your repository — a self-contained
git checkout with its own real, writable `.git` — and runs under an **OS-level
sandbox that blocks writes outside that clone**. The clone is why an agent can
commit, branch, and merge freely without ever touching your working copy; the
sandbox is why a misbehaving or prompt-injected agent can't write anywhere else
on disk. Together they protect your source repository's `.git` (never writable
by an agent), everything on your filesystem outside the agent's workspace, and
Fletch's own state. They deliberately do **not** restrict what an agent can
*read* (under the default engine) or the network it can reach — Fletch confines
writes, it does not virtualize the machine. Isolation is a containment boundary
for live agent processes, not a claim to defeat a determined attacker.

## The two engines

Fletch ships exactly two isolation engines, chosen in Settings. There is no
third mode and no silent fallback between them (`sandbox/engine.rs`,
`sandbox/mod.rs`).

- **Seatbelt (default).** Each agent is launched under a per-agent macOS
  `sandbox-exec` profile. The profile is write-confinement only: it starts from
  `(allow default)`, denies all writes, then re-allows a specific set of paths
  (last-match-wins SBPL). **Reads and network stay open.** The agent runs as
  *you* — same user, same read access, same network — so this is write
  protection, not a virtual machine (`sandbox/seatbelt.rs`).
- **Docker (opt-in).** Each agent process runs in its own
  `docker run --rm --init` container, one container per agent, agent ≈ PID 1.
  The container sees only what Fletch bind-mounts into it, so the rest of your
  filesystem is unreachable *even for reads*. It is truer isolation, at the cost
  of requiring Docker and some startup latency (`sandbox/docker/`).

An agent is stamped with its engine at creation and keeps it for life; changing
the setting only affects agents spawned afterward (`sandbox/mod.rs`). A
Docker-stamped agent whose daemon is unreachable is a **hard error** — Fletch
fails closed rather than quietly launching outside the container boundary the
user selected (`sandbox/mod.rs`, `engine_for`).

### What the seatbelt profile allows

Writes are permitted only to: the agent's workspace root, its private RPC
mailbox, the optional workflow blackboard, the standard scratch/state locations
(`/private/tmp`, `/private/var/folders`, `/private/var/tmp`), each provider's
own on-disk state dirs (so the agent CLI can persist transcripts, config, and
refreshed auth), and `~/.claude.json`. Fletch's own application-data directory
(`fletch.db` — transcripts and settings) is explicitly denied **both read and
write**, so a confined agent can neither exfiltrate nor forge app state
(`sandbox/seatbelt.rs`).

### What the Docker container mounts

Only the agent's writable root and its RPC mailbox are mounted read-write, at
their identical host paths. The provider's config/data dirs are mounted per
provider (Claude's `~/.claude` is read-only except its `.credentials.json` and a
per-agent transcripts dir; the others are read-write). Crucially, the source
repo's `.git` is **never** mounted; only its borrowed git object store is
mounted, and only **read-only** (`sandbox/docker/engine.rs`). There is **no
`--network` flag** anywhere in the launch, so the container uses Docker's default
bridge — the network is open. Containers run as root in v1 (a documented
limitation), which is another reason the container is treated as live-process
containment, not a trust boundary.

## The isolation matrix

Provisioning has two clone strategies, and the engine plus *when* the repo joins
the agent decides which one runs (`sandbox/provision.rs`, `clone_base`).

| Engine | When the repo is added | Clone strategy | Why |
| --- | --- | --- | --- |
| Seatbelt | Always | `git clone --shared` | Same filesystem as the source; borrowed objects are reachable directly, no mount involved, so the cheap shared clone always works. |
| Docker | Present when the container starts (the primary repo, and any repo already tracked at launch) | `git clone --shared` | The borrowed object store is bind-mounted **read-only** at its identical host path when the container launches, so in-container git can read history while writes fail with `Read-only file system`. |
| Docker | Added to an **already-running** container | `git clone --no-hardlinks` | A running container's bind mounts are fixed at `docker run`; a `--shared` clone's borrowed store could never be mounted, so in-container git would fail on missing objects. A self-contained full copy needs no extra mount. |

`git clone --shared` borrows the source's object store through
`.git/objects/info/alternates` and copies **no** objects, so spawning an agent
costs kilobytes and milliseconds instead of a full history copy. New objects
(the agent's commits and fetches) land in the clone's own store; reads of
existing history fall through to the borrowed one. Borrowed objects are only
ever *referenced*, never written, and under Docker the store is mounted
read-only — so a container can't reach back through the alternates link to
mutate the source (`sandbox/provision.rs`). `git clone --no-hardlinks` makes a
full, self-contained copy with no alternates and no shared inodes.

After either clone, three fixups run (`sandbox/provision.rs`):

- **Origin is rewritten** to the source repo's *real* remote, so push/PR/fetch
  behave as they would from a normal checkout. If the source has no remote, the
  clone's implicit local-path `origin` is *removed* — otherwise a push could
  silently create branches and objects inside your source repo.
- **Delegation git hooks** (`post-commit`, `post-merge`) are installed into the
  clone so the agent's own native git commits and merges signal the UI. They are
  installed only in the clone; host-side git runs with hooks disabled, so they
  never fire on the host.
- **A git identity is frozen** into the clone's own config, so in-container git
  (which can't see your global gitconfig) can commit without failing on "Please
  tell me who you are."

## What an agent can and cannot do

Stated plainly, because the trust anchor earns trust by naming its limits.

**An agent CAN:**

- **Read your disk — under seatbelt.** The seatbelt profile confines writes, not
  reads; the agent runs as your user and can read anything you can. Under Docker
  it can read only what is mounted into the container.
- **Open network connections — under both engines.** Neither engine restricts
  the network. Seatbelt's `(allow default)` leaves it open; the Docker launch
  sets no `--network` flag, so the container gets the default bridge. Treat every
  agent as network-capable.

**An agent CANNOT:**

- **Write outside its workspace.** Both engines deny it: seatbelt by policy,
  Docker by mounting nothing else writable.
- **Touch your source repo's `.git`.** The agent works in a clone with its own
  `.git`; your repository's `.git` is never writable by an agent, and under
  Docker it is never mounted at all.
- **Hold your GitHub credentials.** Pushes and PR creation never run inside the
  sandbox. They are brokered: the agent writes a request to its RPC mailbox, and
  Fletch runs the operation **host-side**, where the credentials live
  (`rpc/git.rs`, ops `git_push`, `open_pr`, `git_fetch`). Credentials never enter
  the sandbox.

**The one caveat to state loudly:** the brokered `git_push` and `open_pr` ops
currently run **without a confirmation prompt** (`rpc/git.rs`). An agent that
decides to push a branch or open a PR can do so under your GitHub identity
without asking first. Credentials stay out of the sandbox, but publication is
not gated. Choose the tasks and repositories you point agents at accordingly.
(The README states this same caveat; keep the two in sync.)

## Why not linked git worktrees

A natural-seeming alternative to cloning is `git worktree add` — a second
working tree that shares the origin repository's object store and ref database.
Fletch rejects it, for two reasons (`sandbox/provision.rs`):

1. **It's incompatible with the Docker engine.** A linked worktree's `.git` is a
   *file* that points at the origin repo's `.git/worktrees/<name>` by absolute
   path. Containerizing such a checkout would require bind-mounting your real
   `.git` into the container — and a writable `.git/hooks` there is a host escape
   (the next `git` command you run on the host would execute agent-authored hook
   code). The source `.git` must never enter an agent's sandbox writable.
2. **Agents need a real, writable `.git`.** Local git mutations (commit, merge,
   conflict resolution) now run as native git *inside* the sandbox, which
   requires a genuine writable object store and config. A clone has one; a linked
   worktree borrows the origin's, which is exactly what must stay out of reach.

Both engines therefore converge on the same shape: a self-contained clone.

## Why not a VM

Neither engine is a virtual machine, and Fletch does not claim to be one.

- **Seatbelt** is a write-confinement profile applied to a process that still
  runs as *you*, on your kernel, with your read access and your network. It stops
  writes; it does not virtualize anything.
- **Docker** is live-process containment: it isolates the filesystem an agent can
  see and write, but the network is open, and in v1 containers run as root, so
  in-container privilege isolation is not relied upon.

The real review gate that keeps agent output off your main branch is not the
sandbox at all — it's the clone-plus-PR flow with you in the loop: nothing merges
without your review.

## Terminology

Fletch uses precise words for these concepts. Please use them consistently.

- **Workspace** — the agent's isolated area on disk (`~/.fletch/workspaces/<id>/`),
  containing its clone (or one clone per repo, for a multi-repo agent). This is
  the user-facing word for "where the agent works."
- **Clone** — the isolation *mechanism*: each workspace is a `git clone` of your
  repository, never a linked worktree. When you need one word for how Fletch
  isolates an agent, it is "clone."
- **Git worktrees are not the isolation mechanism.** Fletch does not use
  `git worktree add` to isolate agents (see [Why not linked git worktrees](#why-not-linked-git-worktrees)).
  Two internal uses of the word "worktree" survive and are *not* user-facing
  terminology for isolation:
  - The **`worktrees` database table** is legacy-internal naming: one row per
    repo checkout in a workspace (`database.rs`). Renaming the table is churn we
    haven't taken on; it does not mean agents use git worktrees.
  - The **workflow merge stage** does use a genuine linked worktree — but of an
    internal, disposable *run repository* under `~/.fletch/runs/`, purely to
    integrate parallel step results (`workflow/gitops.rs`). It never touches your
    source repository, and it is unrelated to how agents are isolated.

The historical spelling `~/.fletch/worktrees/` for the on-disk root was renamed
to `~/.fletch/workspaces/`; a one-time migration handles existing installs
(`workspace.rs`).

## Authoritative code

The code is canonical. When this document and the code disagree, the code wins —
and this document should be corrected.

- `src-tauri/src/sandbox/seatbelt.rs` — the macOS `sandbox-exec` profile: the
  deny-writes-then-re-allow model, the writable set, and the app-data deny.
- `src-tauri/src/sandbox/docker/` — the Docker engine: per-agent
  `docker run --rm --init`, the identical-host-path mounts, the read-only
  borrowed object store, and the invariants that keep the source `.git` and your
  credentials out of the container.
- `src-tauri/src/sandbox/provision.rs` — how a workspace comes into existence:
  the two clone strategies, origin rewrite, hook install, and identity seeding.
- `src-tauri/src/rpc/git.rs` — the host-side broker for `git_push`, `open_pr`,
  and `git_fetch`, including the (currently unprompted) publication ops.
</content>
</invoke>
