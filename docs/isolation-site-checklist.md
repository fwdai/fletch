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
- **Why:** provisioning is `git clone --shared` on the host, or a full copy
  pushed into a box's guest. Linked worktrees are explicitly
  rejected — see `docs/isolation.md`, "Why not linked git worktrees."

### 2. Two isolation engines, no third mode, no fallback

- **Must say:** Fletch ships **two** isolation engines — a **Linux VM** per
  agent (the box) wherever the Mac can run one, and macOS **Seatbelt** for
  what stays on the Mac (a macOS app's project, a provider the VM doesn't run,
  the first workspaces while the VM image downloads). Fletch picks per
  workspace; nothing to install. A workspace placed in a VM that can't have one
  **fails closed** — it does not silently fall back to seatbelt. Docker and
  Podman are not supported.
- **Stale if it says:** Docker or Podman is an isolation engine, optional or
  required; there is only one sandbox; an unavailable engine degrades
  gracefully to another.
- **Why:** `EngineKind { SandboxExec, Box }`; `engine_for` hard-errors on a box
  that cannot boot.

### 3. Seatbelt is not a VM

- **Must say:** Seatbelt is OS-level **write confinement** for a process that
  runs as your user; the box is a VM.
- **Stale if it says:** every agent runs in a VM; Seatbelt is a full
  virtualization boundary; the sandbox defeats a determined attacker.
- **Why:** `sandbox/seatbelt.rs` (write-only confinement, `allow default`);
  `sandbox/boxes/` (a libkrun micro-VM per agent).

### 4. Network is open under seatbelt (honesty item)

- **Must say:** under **Seatbelt** the **network is open**; treat those agents
  as network-capable. A box reaches the network only through the host proxy's
  allow-list.
- **Stale if it says:** every agent is network-isolated; or every agent has
  open network access.
- **Why:** seatbelt `(allow default)`; a box has no network device, only the
  egress proxy (`sandbox/guarantees.rs`).

### 5. Reads are open under seatbelt

- **Must say:** the default (seatbelt) engine confines **writes**, not reads —
  an agent can read anything you can. A box reads only its own disks and the
  folders granted to the workspace.
- **Stale if it says:** the default sandbox prevents agents from reading your
  files; reads are sandboxed under seatbelt.
- **Why:** `sandbox/seatbelt.rs` — the profile only denies `file-write*` (plus a
  read+write deny on Fletch's own data dir); everything else is readable via
  `allow default`.

### 6. Your source repo's `.git` is never writable

- **Must say:** an agent can never write your repository's `.git`. It works in a
  clone with its own `.git`; a box holds a full copy and no mount of yours.
- **Stale if it says:** agents share your repo's git store writably; worktrees
  share the origin `.git`.
- **Why:** `sandbox/provision.rs`; `sandbox/boxes/provision.rs`.

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
**clone (not worktree) + two engines (a Linux VM where possible, seatbelt
otherwise, fail closed) + seatbelt is not a VM + network and reads open under
seatbelt +
source `.git` never writable + credentials brokered host-side but push/PR
unprompted.** If any surface tells a different story, it is stale.
</content>
