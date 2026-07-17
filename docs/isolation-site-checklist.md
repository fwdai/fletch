# Isolation site-correction checklist

The external site and marketing surfaces (**fletch.sh**, its `/docs`, and
`llms.txt`) live outside this repository, so this repo can't edit them. This
checklist enumerates the claims those surfaces must align to. The canonical
source is [`docs/isolation.md`](./isolation.md) in this repo; if the site
disagrees with the code, the code wins and the site is stale.

Each item is phrased as **"the site must say X; if it says Y, it's stale"** so a
person or agent with site access can execute it mechanically. Prior analysis
found `llms.txt` describing isolation as a **"worktree"** — treat that as the
**primary known error** and hunt every instance of it first.

## How to execute

1. Grep the site content (pages, MDX, `llms.txt`, meta descriptions, OG copy)
   for: `worktree`, `work tree`, `No Docker`, `no containers`, `VM`, `virtual
   machine`, `network`, `credential`, `token`.
2. For each hit, apply the matching item below.
3. Verify nothing else claims an isolation model Fletch doesn't implement (e.g.
   "runs in a VM", "fully sandboxed reads", "confirms before pushing").

## The claims the site must align to

### 1. Terminology: clone, not worktree (primary known error)

- **Must say:** each agent works in an isolated **clone** of your repo (a
  self-contained git checkout with its own writable `.git`); the on-disk root is
  `~/.fletch/workspaces/<id>/`.
- **Stale if it says:** each agent gets a **git worktree**; Fletch uses linked
  worktrees; the path is `~/.fletch/worktrees/<name>/`.
- **Why:** provisioning is `git clone` (`--shared` normally, `--no-hardlinks`
  for repos added to a live Docker container). Linked worktrees are explicitly
  rejected — see `docs/isolation.md`, "Why not linked git worktrees."

### 2. Two isolation engines, no third mode, no fallback

- **Must say:** Fletch ships **two** isolation engines — macOS **Seatbelt**
  (default) and **Docker** (opt-in). Docker is optional, not required. If Docker
  is selected but unavailable, spawning **fails closed** — it does not silently
  fall back to seatbelt.
- **Stale if it says:** "No Docker, no containers" as a blanket statement;
  Docker is required; there is only one sandbox; an unavailable engine degrades
  gracefully to another.
- **Why:** `EngineKind { SandboxExec, Docker }`; `engine_for` hard-errors on an
  unavailable Docker daemon.

### 3. Not a VM (both engines)

- **Must say:** neither engine is a virtual machine. Seatbelt is OS-level
  **write confinement** for a process that runs as your user; Docker is
  **per-agent live-process containment** (and runs as root in v1). No VM is
  required to run Fletch.
- **Stale if it says:** agents run in a VM; the sandbox is a full virtualization
  boundary; it defeats a determined attacker.
- **Why:** `sandbox/seatbelt.rs` (write-only confinement, `allow default`);
  `sandbox/docker/engine.rs` (threat-model note: containment, not a trust
  boundary; root in v1).

### 4. Network is open (honesty item)

- **Must say:** under **both** engines the **network is open**. Seatbelt leaves
  it open; the Docker launch sets no network restriction. Treat agents as
  network-capable.
- **Stale if it says:** agents are network-isolated; the sandbox blocks network
  access; egress is restricted.
- **Why:** seatbelt `(allow default)`; the Docker run has no `--network` flag,
  so the container uses the default bridge.

### 5. Reads are open under seatbelt

- **Must say:** the default (seatbelt) engine confines **writes**, not reads —
  an agent can read anything you can. Docker additionally restricts reads to
  what is mounted.
- **Stale if it says:** the default sandbox prevents agents from reading your
  files; reads are sandboxed under seatbelt.
- **Why:** `sandbox/seatbelt.rs` — the profile only denies `file-write*` (plus a
  read+write deny on Fletch's own data dir); everything else is readable via
  `allow default`.

### 6. Your source repo's `.git` is never writable

- **Must say:** an agent can never write your repository's `.git`. It works in a
  clone with its own `.git`; under Docker your source `.git` is never mounted,
  and the borrowed object store it does mount is **read-only**.
- **Stale if it says:** agents share your repo's git store writably; worktrees
  share the origin `.git`.
- **Why:** `sandbox/provision.rs`; `sandbox/docker/engine.rs` (object store
  mounted `:ro`, source `.git` never mounted).

### 7. Credential brokering, with the no-confirmation caveat

- **Must say:** GitHub credentials **never enter the sandbox**. Pushes and PRs
  are **brokered host-side** through an RPC mailbox. State this candidly: those
  publication ops (`git_push`, `open_pr`) currently run **without a confirmation
  prompt**, so an agent can push or open a PR under your identity without asking
  — choose tasks and repos accordingly.
- **Stale if it says:** agents hold your GitHub token; the sandbox has network
  credentials; **or** that Fletch asks for confirmation before every push/PR
  (it does not, today — don't overclaim safety).
- **Why:** `rpc/git.rs` — host-side broker; no prompt gate on the mutating ops.

## Consistency check

After edits, the site's isolation story must match, in one voice:
**clone (not worktree) + two engines (seatbelt default, docker opt-in, fail
closed) + not a VM + network open under both + reads open under seatbelt +
source `.git` never writable + credentials brokered host-side but push/PR
unprompted.** If any surface tells a different story, it is stale.
</content>
