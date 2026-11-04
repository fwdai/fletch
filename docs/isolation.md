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
*read* or the network it can reach under seatbelt, which confines writes and
does not virtualize the machine; the box does both (below). Isolation is a containment boundary
for live agent processes, not a claim to defeat a determined attacker.

## The two engines

Fletch ships exactly two isolation engines, chosen per workspace (below).
There is no third mode and no silent fallback between them
(`sandbox/engine.rs`, `sandbox/mod.rs`).

- **Seatbelt.** Each agent is launched under a per-agent macOS
  `sandbox-exec` profile. The profile is write-confinement only: it starts from
  `(allow default)`, denies all writes, then re-allows a specific set of paths
  (last-match-wins SBPL). **Reads and network stay open.** The agent runs as
  *you* — same user, same read access, same network — so this is write
  protection, not a virtual machine (`sandbox/seatbelt.rs`).
- **The box.** Each agent runs in its own Linux micro-VM, its checkout inside
  the guest. Nothing of your machine is in the guest but the folders you grant
  the workspace, so the rest of your filesystem is unreachable *even for
  reads*, and egress goes only through the host proxy's allow-list
  (`sandbox/boxes/`, `docs/box.md`).

An agent is stamped with its engine at creation and keeps it for life
(`sandbox/mod.rs`). A box-stamped agent on a machine that cannot boot one is a
**hard error** — Fletch fails closed rather than quietly launching it under
seatbelt (`sandbox/mod.rs`, `engine_for`). A workspace stamped with an engine
Fletch no longer ships (Docker, Podman) refuses to start, resume or fork rather
than moving to another engine (`sandbox::ensure_engine_not_retired`).

Which engine a new agent gets is decided once, at creation, by
`sandbox::engine_for_spawn` from two inputs: the project's placement
(`sandbox.placement`: automatic, the macOS sandbox or a Linux VM; under
automatic a Linux VM when one can boot right now, the macOS sandbox
otherwise, and a repository that builds a macOS or iOS app stays in the
macOS sandbox), and an optional per-session choice made in the composer for a
new agent (the macOS sandbox or a Linux VM), which stands over the placement
for that agent alone. There is no global engine setting; off macOS there is
no seatbelt, so every new agent there is a box. A
fork's child is stamped with exactly the engine its parent was stamped with,
whatever the placement now says, so a parent kept on this Mac never forks into a VM. A
per-session Linux VM that cannot be had (a runtime that cannot boot, a
provider boxes do not run) fails the spawn with that reason
rather than landing somewhere else; the composer offers the choice from the
same answer (`available_workspace_types`). An agent that adopts an existing
tree (the local reviewer, a workflow step in its run's workspace) runs where
that tree lives whatever was chosen. The result is stamped on the record and
kept for life.

### What the seatbelt profile allows

Writes are permitted only to: the agent's workspace root, its private RPC
mailbox, the optional workflow blackboard, the standard scratch/state locations
(`/private/tmp`, `/private/var/folders`, `/private/var/tmp`), each provider's
own on-disk state dirs (so the agent CLI can persist transcripts, config, and
refreshed auth), `~/.claude.json`, and its project's toolchain cache. Fletch's
own application-data directory (`fletch.db` — transcripts and settings) is
explicitly denied **both read and write**, so a confined agent can neither
exfiltrate nor forge app state (`sandbox/seatbelt.rs`).

One read deny is there for the user's sake rather than the boundary's. macOS
asks before an app — or any process it spawned, which an agent is — first reads
`~/Downloads`, `~/Desktop`, `~/Documents`, iCloud Drive (`~/Library/Mobile
Documents`) or another app's container (`~/Library/Containers`, `~/Library/Group
Containers`), and an agent's first `find ~` used to raise every one of those
prompts on turn one, with nothing in Fletch wanting any of them. The agent
profile denies those folders by name (`policy::MACOS_PRIVACY_DIRS`): a sandbox
deny is a plain `EPERM`, no prompt, and the agent skips the folder as it skips
any unreadable one. What legitimately lives inside is given back: the launch's
own roots read-write when a host keeps its data dir there, and the user's source
repositories read-only, since a checkout's `--shared` clone borrows their
objects. A file attached from such a folder is copied into the agent's workspace
at drop time instead of being read in place, so macOS asks — if it asks — while
the user is attaching a file from that folder. The Run profile does not carry
the deny: a dev server does not walk the home directory.

The toolchain cache (`~/.fletch/toolchain-cache/<project id>`, or
`~/.fletch/dev/toolchain-cache/<project id>` for a debug build) is where every
package manager, the cargo overlay and the headless browsers are redirected. What is in it gets executed by later
sandboxed processes, so it is keyed by project: every profile names that
project's root and never the directory above it, so no project
can write what another one executes. A box points the same variables at the
same path, which by path identity is a directory on its own guest disk: the
package caches without the cargo overlay, and not shared with the host's
cache. Why it is shaped that way (per project, per
build, platform-keyed rows, the reclaim, the nested instance and `fletch-host`)
is told once, in the module doc of `sandbox/toolchain_cache/mod.rs`. The Run
profile denies the host's claude and codex login files as the agent profile
does; it leaves the Keychain open, a separate decision.

One carve-out runs the other way, inside the writable checkout: each
repository's **git-executable configuration** — `.git/config`,
`.git/config.worktree`, `.git/hooks/`, `.git/info/` — is denied write
(`sandbox/policy.rs`, invariant 3). Git resolves hooks, clean/smudge filters,
textconv programs and merge drivers from those paths, so an agent-writable
`.git/config` is a *host* code-execution primitive: Fletch runs git on the
checkout constantly (diff polling, workflow boundary commits), and a planted
`filter.*.clean` executes on the next `git add`. The
tracked `.gitattributes` stays writable — it is source an agent legitimately
edits, and an attribute naming a driver is inert while it cannot define one.
Independently, every host-side git invocation carries hardening overrides that
neutralise the fixed-name equivalents (`git/hardening.rs`), applied at git's
single spawn seam rather than per call site.

Withholding the write is **seatbelt-only**, and a path rule is only as good as
the paths it names. So a second, **engine-independent** check backs it up: before running git in an agent checkout, Fletch reads that checkout's config
and **refuses if any setting would execute a program** — matched on
`(section, leaf)` with the subsection ignored, so `filter.<any-name>.clean` is
caught even though the driver name is chosen by the agent's own `.gitattributes`
and can never be enumerated (`git/hardening.rs`). Keys the `-c` overrides
already neutralise (`core.hooksPath`, `core.fsmonitor`, the pager and editors)
are *not* refused: they can never reach host-side git, and refusing them bricked
every checkout whose `npm install` ran a husky-style `prepare` script. It reads
whatever config git is about to read, so it doesn't care how that config got
there. It reads via
`--show-scope` and considers the **local and worktree** scopes, because
`.git/config.worktree` (honoured when `extensions.worktreeConfig=true`) is as
agent-writable as `.git/config` — a key smuggled there was once invisible to a
`--local` listing. Scoped to agent checkouts: a user's own repository legitimately
carries these keys — husky sets `core.hooksPath`, git-lfs sets `filter.lfs.*` —
and isn't agent-writable anyway, and its `global`/`system` config is skipped.
A refused checkout is not a dead end: the Git panel names the offending keys and,
on an explicit click, unsets them from the checkout's own config and re-reads it
— a key it cannot reach (one pulled in through `include.path`) still fails,
naming what is left (`git/hardening.rs`, `remove_steerable_config`).

The refusal covers every command that can trigger an executable setting: the
`git::cmd` helper seam that `run_git`/`git_output` funnel through, plus the paths
that build a git command directly *and* run a trigger — `git_state`'s three public
reads (status/diff/numstat run clean filters and textconv), `pull` (a merge runs
merge drivers), and the push/fetch transport paths (`push`, `push_head_to_branch`,
`fetch_fork_point`, `provision::fetch`). push/fetch run no filter or merge driver,
but they run the *transport*-executing keys `core.sshCommand`, `core.gitProxy` and
`remote.<name>.uploadpack`/`receivepack` over ssh/local transport — and the `-c`
overrides omit those (they carry a user's real auth), so the refusal is the only
layer that covers them there. `fetch_base` fetches the user-owned source repo,
outside `checkouts_root`, so the scoped refusal is a no-op there.

One limit to state plainly: the **Run panel deliberately carries neither half** —
`npm install` on a husky project legitimately writes `core.hooksPath`, and Run is
already the weaker boundary by design.

The same shape holds for **files Fletch itself writes into a tree the agent can
also write**: the `.codegraph` index and its `.git/info/exclude` entry, the
attribution `commit-msg` hook, the session profile (`.fletch-profile`: MCP
config, skills), the RPC mailbox, the codex overlay.
The host writes with the user's full rights, so a link the agent planted at, or
anywhere above, such a file would carry the write out of the sandbox, and a path
deny cannot stop that on its own: renaming `.git` to a name the rules don't
mention and linking it back walks around them. So these writes go **by handle,
never by path**: every directory is opened without following a link, and the
file is replaced in one rename, which replaces a planted link instead of writing
through it (`agent/credential_file.rs`, `PrivateDir`). The same goes for what
the host reads and deletes there: the mailbox's requests are listed, read and
removed relative to handles on `requests/` and `responses/`, so a `requests`
swapped for a link is not served, and a read refuses anything but a regular
file (a link, or a FIFO that would block the host). For a checkout that is
`exec::Executor::write_beneath`, which in a box is the guest's own write, since
a link there leads nowhere outside the box. The file panel's Save is the one
host write into a checkout that follows links, because repositories
legitimately contain them; it checks that the resolved path stays inside the
checkout instead (`commands/files.rs`, `safe_join`).

## The isolation matrix

A seatbelt checkout is a `git clone --shared` of the source on the host
(`sandbox/provision.rs`, `clone_base`): same filesystem as the source, so the
borrowed objects are reachable directly.

`git clone --shared` borrows the source's object store through
`.git/objects/info/alternates` and copies **no** objects, so spawning an agent
costs kilobytes and milliseconds instead of a full history copy. New objects
(the agent's commits and fetches) land in the clone's own store; reads of
existing history fall through to the borrowed one. Borrowed objects are only
ever *referenced*, never written (`sandbox/provision.rs`).

A box delivers its base differently: `git init` in the guest and a push of the
base commit over the box's channel, a full copy (`sandbox/boxes/provision.rs`).
After the base is in, every checkout Fletch makes (a host clone, a box's guest
checkout, a restored one) runs the same pipeline,
`sandbox::provision::fixups`. Each step is git in the checkout or a write
through its executor, so it runs wherever the checkout lives; a failure removes
the half-built checkout there. In order:

- **Origin is rewritten** to the source repo's *real* remote (added in a box's
  fresh repository, which has none), so push/PR/fetch behave as they would from
  a normal checkout. If the source has no remote, the clone's implicit
  local-path `origin` is *removed* — otherwise a push could silently create
  branches and objects inside your source repo.
- **A git identity is frozen** into the checkout's own config, so git in a box
  (which can't see your global gitconfig) can commit without failing on "Please
  tell me who you are."
- **Commit signing is switched off** in the checkout's own config
  (`commit.gpgsign=false`). The seatbelt engine, unlike a box, lets git
  read your global gitconfig, so a `commit.gpgsign=true` there would reach the
  sandbox while the key it names (`~/.ssh`, the ssh-agent, a gpg keyring) stays
  denied — and every agent commit would fail with "failed to write commit
  object". Your own repos keep signing; the clone's local scope outranks global.
  Host-side git never signs implicitly in a checkout either (every invocation
  inside an agent-writable tree carries `-c commit.gpgsign=false`): implicit
  signing reads the key and its helpers — `user.signingKey`,
  `gpg.ssh.defaultKeyCommand`, a *program* — from the checkout's config, which
  is the agent's to write. In your own repositories Fletch's git signs as your
  config asks, so the first commit of a new project is signed like any of
  yours.
- **Git hooks** are written, executable, into the checkout's `.git/hooks`
  through its executor: the delegation hooks (`post-commit`, `post-merge`), so
  the agent's own native git commits and merges signal the UI, and the
  attribution hook (`commit-msg`). Host-side git runs with hooks disabled, so
  they never fire on the host.
- **The caller's checkout step** puts HEAD where it belongs: detached at the
  base for a fresh workspace, the restore ladders for a restored one (fetching
  inside the step), the pushed base for a box.
- **Local heads and `origin/HEAD` are reconciled**: a head the clone inherited
  from the source is re-pointed at its fetched `origin/<name>` or deleted (a
  box's fresh repository has none), and `origin/HEAD` names the source's real
  default branch.

A workflow run repository (`workflow/gitops.rs`) takes only the origin rewrite,
the identity and the attribution hook from this pipeline: its host-side
boundary commits keep your signing config, and nothing listens for delegation
hooks there.

### Signing on publish

When **your repository** signs its commits (`commit.gpgsign=true` as your own
`git commit` there would see it), Fletch signs agent work at the moment it is
published. Every push Fletch makes — the agent's `git_push` and `open_pr` ops,
the Git panel's Push, a workflow run's finalize, a new project's first
publish — goes through one publication layer (`git::publish`), so none can
skip it. For work made in a checkout it replaces the commits no remote has yet
with signed copies (same tree, parents, author, committer and message), moves
the branch to the signed tip and pushes that. Published history keeps its
hashes.
Your own repository, when it is what gets pushed (a new project's first
publish), goes out exactly as it stands: its commits already carry whatever
signature your git gave them, and its history is yours, other committers and
all.

- **The signer runs in your repository, not the checkout.** `commit-tree -S`
  reads its key and helper programs from the repo it runs in, so it runs in the
  user-owned repo the checkout was cloned from — which no agent can write —
  with the checkout supplying objects only. Nothing in a checkout's
  `.git/config` can choose the key or run a command on the host.
- **What is published is asked of the remotes.** A signed publish stops at
  the branch tips your fetch and push remotes advertise over the wire right
  then, all fetched first: what any of them has is never re-signed, and no ref
  in the checkout can pass an unsigned commit off as published. The
  own-committer check runs after that refresh, so signing never reaches a
  commit it did not.
- **Every commit in the range is signed afresh.** The checkout's `.git` is
  agent-writable, so a signature header already on a commit is only a claim.
- The signature is made where the key lives and at the moment you approve the
  push, so it vouches for exactly what you let out. A key the host cannot use
  fails the push with the reason rather than publishing unsigned.
- **Host-side git finds your tools the way your terminal does.** An app
  launched from the Dock inherits launchd's sparse environment, not your
  shell's. Host-side git carries `PATH` from your login shell everywhere, and
  the credentials `SSH_AUTH_SOCK` and `GNUPGHOME` only in **your own
  repository** — where the signer runs and Fletch's own commits are made —
  never in an agent-writable checkout, whatever the app process holds
  (`git_dist::helper_env`). So a Homebrew `gpg`, a key held by an agent your
  shell rc starts (1Password's, say) and a relocated keyring work from Fletch
  as they do from a terminal. A value your shell exports blank counts as
  unset, not inherited from the app.
- **Host-side transport reads none of the checkout's configuration.** A
  checkout's `.git` is the agent's to write: its config, its submodule configs
  under `.git/modules`, its refs. Every fetch, push and ls-remote the host
  runs for a checkout — the agent's `git_fetch`, which needs no approval, the
  publish, the Git panel's pull, the restore-time recovery fetches — runs in
  **your repository**, with your configuration (your remote and push URLs,
  `insteadOf` rewrites, proxies, credential helpers), over the checkout's
  objects, with per-worktree refs in the checkout (`git::remote::Transport`,
  on git's own worktree split). It names explicit URLs from your config, so
  git keeps no tracking refs in your repository; the checkout's `origin/*` is
  set afterwards, for the agent's own use. Every decision that depends on a
  remote's state is read from the remote over the wire, never from a ref in
  the checkout. Host-side git never recurses into submodules. As policy, the
  checkout's `origin` must still name your repository's remote: a checkout
  an agent repointed is refused outright.
- **Pushes, leases and pulls behave as your git's would.** A push goes to
  every push URL you configured; a force needs exactly one, since its lease
  can hold one remote's tip, and expects the tip that remote reports just
  before the force — refused unless this checkout integrated it, as
  `--force-if-includes` would. The Git panel's pull fetches this way and then
  joins the histories with git's own `pull`, so your `pull.rebase` and
  `pull.ff` still decide how.

### Why signing works this way

A signature is a statement by whoever holds the key, so the design follows
from who is taking responsibility for an agent's commits, and when. Fletch's
answer is that the user is responsible and the agent is a tool: commits carry
the user's identity, the host refuses a foreign committer, and the user
approves each publish. The approval is the honest moment for the user's key to
vouch, and it is the one place the key is reachable. Alternatives considered:

- **Your key, or your ssh-agent, in the sandbox.** Rejected: an agent could
  sign anything, at any time, and push over SSH past every host-side guard.
  This is the hole the credential denies closed.
- **A host-brokered `git commit` op.** Rejected: every commit becomes an RPC,
  "local git is yours" stops being true, and the signature vouches for
  intermediate commits nobody has looked at.
- **A signing key per agent, in the sandbox** — the agent as a co-worker with
  its own identity. Rejected for now: it is only coherent if the agent has an
  identity GitHub recognises. A key registered on your account verifies as
  you, which is the first option with extra steps; a bot account or GitHub App
  means a different author and committer, so the frozen identity, the
  own-committer push check and the attribution hook would all change. Worth
  reopening if agents gain a GitHub identity of their own, for example on a
  host shared by several people.

## What an agent can and cannot do

Stated plainly, because the trust anchor earns trust by naming its limits.

Each claim below is also a value in `sandbox/guarantees.rs`, carrying its
**per-engine coverage** — `enforced`, `partial`, or `unenforced` with the reason.
That matrix, not this prose, is authoritative for *which engine* delivers what:
the engines are not equally capable and neither is a superset of the other, so
"which engine am I on" does not by itself answer "what am I protected from". The
compiler forces every claim to declare coverage for every engine, so the gap
cannot go quietly unstated the way a doc caveat can. The
`describe_sandbox_isolation` command returns the matrix for display.

**An agent CAN:**

- **Read your disk — under seatbelt.** The seatbelt profile confines writes, not
  reads; the agent runs as your user and can read anything you can, except the
  carve-outs the profile denies by name: Fletch's app data, the host's provider
  logins, `~/.ssh`, the `gh` login, git's stored credentials, the Keychain, and
  the folders macOS guards with a privacy prompt (see below). In a box it can
  read only its own disks and the folders you granted the workspace.
- **Open network connections — under seatbelt.** Seatbelt's `(allow default)`
  leaves the network open; treat every seatbelt agent as network-capable. A box
  has no network device: its egress goes only through the host proxy, on ports
  80 and 443, under the posture the user picked (Settings › Sandbox › Network,
  `docs/box.md` "Network"). The default posture is open, so by default a box
  agent reaches any host too, with every connection recorded; `ask` holds a
  new host for the user's answer, and `allowlist` refuses it.

**An agent CANNOT:**

- **Write outside its workspace.** Both engines deny it: seatbelt by policy,
  a box by having no host folder but the ones you granted.
- **Touch your source repo's `.git`.** The agent works in a clone with its own
  `.git`; your repository's `.git` is never writable by an agent, and a box
  holds a full copy with no mount of it at all.
- **Hold your GitHub credentials.** Pushes and PR creation never run inside the
  sandbox. They are brokered: the agent writes a request to its RPC mailbox, and
  Fletch runs the operation **host-side**, where the credentials live
  (`rpc/git.rs`, ops `git_push`, `open_pr`, `git_fetch`). Credentials never enter
  the sandbox. Under seatbelt the profile also withholds your own, at their
  standard locations: `~/.ssh` and the launchd ssh-agent socket (`SSH_AUTH_SOCK`
  is dropped from the agent's env), the `gh` CLI login (`~/.config/gh`, plus
  `$XDG_CONFIG_HOME/gh` and `$GH_CONFIG_DIR` when set), git's stored credentials
  (`~/.git-credentials`, `~/.config/git/credentials`, plus
  `$XDG_CONFIG_HOME/git/credentials`) and git's credential-cache socket
  (`~/.cache/git/credential/socket`, `~/.git-credential-cache/socket`, plus
  `$XDG_CACHE_HOME/git/credential/socket`). The home defaults are denied whether
  or not a relocation is set: an agent can change those variables in its own
  shell, and older files at the defaults would otherwise still work.
  `GH_TOKEN` and `GITHUB_TOKEN` are stripped from the agent's env. Every sandbox
  clone, and a workflow's run repo, gets a GitHub SSH origin rewritten to HTTPS,
  so git inside the sandbox has no credential to publish with. Not covered: a
  credential store or cache socket at a path only your git config names, or an
  ssh-agent at another socket path (1Password, gpg-agent).

**What publication *is* gated by.** Every brokered op is checked against the
agent's **capability grant**, stamped when it spawns and never re-read, so a
later policy change can't widen an agent already running (`rpc/caps.rs`):

- **No agent can push the branch its work is reviewed against**, nor a
  conventional trunk (`main`, `master`) whatever a repo declares as its base.
  Before this grant existed `git_push` validated nothing, so an agent sitting on
  the base pushed straight onto it.
- **No agent can *force*-push a branch other than its own.** A
  `--force-with-lease` push rewrites remote history, and the lease passes for any
  branch the agent just fetched — so with only the rule above (which fences the
  review base and the trunks, nothing else) an agent could fetch a shared branch
  (`develop`, a `release/*`, a teammate's), `checkout -B` its HEAD onto it, and
  force-overwrite it under your identity. Force is therefore confined to the
  agent's own recorded work branch (`AgentRecord.repos[].branch`) — the
  spawn-stable name it cannot repoint the way it can its live `HEAD`. Until that
  branch has been recorded (a fresh checkout that hasn't materialized one yet),
  force fails **closed**; a non-force push is unaffected and may still create a
  branch (`rpc/caps.rs`, `refuses_force`).
- **A workflow step agent cannot publish at all.** A run publishes through its
  own `wf/`-guarded finalize; a step publishing directly would bypass that guard.

**Approving the act itself** is available but **off by default**
(`rpc/approval.rs`, Settings › Git › "Ask before publishing"). Turned on,
every push and PR waits for you, naming the specific act ("push fix/login"), and
anything that isn't an explicit approval — no window listening, a dismissed
prompt, nobody answering within the configured wait (Settings › Git › "Approval
wait", 120s by default) — resolves to *denied*, so the gate cannot fail open.

Turning it on does not disturb what you already asked for. A push from a checkout
enrolled in autopilot, and any publish covered by a Git-panel action you clicked,
are recognised as already authorized and approved by the host without a prompt
(`autopilot::pre_authorizes`, `supervisor::delegation::pre_authorizes`) — so an
unattended run keeps running and a button press doesn't ask twice. Only a publish the agent decided on by itself prompts.

It is off by default as a product choice rather than a technical limit, and it is
worth being clear that it is a *secondary* control: with network egress
unrestricted (see below), an agent never needed `git_push` to exfiltrate. What the
gate protects is attribution — a branch or pull request appearing to come from you.

**So the caveat that remains, by default:** for its *own* branch, an agent pushes
and opens a pull request under your GitHub identity without asking. Credentials
stay out of the sandbox and the destination is constrained, but the act is not
gated unless you enable it. Choose the tasks and repositories you point agents at
accordingly. (The README states this same caveat; keep the two in sync.)

## Why not linked git worktrees

A natural-seeming alternative to cloning is `git worktree add` — a second
working tree that shares the origin repository's object store and ref database.
Fletch rejects it, for two reasons (`sandbox/provision.rs`):

1. **It would put your real `.git` in reach.** A linked worktree's `.git` is a
   *file* that points at the origin repo's `.git/worktrees/<name>` by absolute
   path. Sandboxing such a checkout would require making your real `.git`
   writable to the agent — and a writable `.git/hooks` there is a host escape
   (the next `git` command you run on the host would execute agent-authored hook
   code). The source `.git` must never enter an agent's sandbox writable.
2. **Agents need a real, writable `.git`.** Local git mutations (commit, merge,
   conflict resolution) now run as native git *inside* the sandbox, which
   requires a genuine writable object store and config. A clone has one; a linked
   worktree borrows the origin's, which is exactly what must stay out of reach.

Both engines therefore converge on the same shape: a self-contained clone.

## Seatbelt is not a VM

- **Seatbelt** is a write-confinement profile applied to a process that still
  runs as *you*, on your kernel, with your read access and your network. It stops
  writes; it does not virtualize anything.
- **The box** is a VM, which is why a workspace gets one wherever the Mac can
  run it.

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
  One internal use of the word "worktree" survives and is *not* user-facing
  terminology for isolation: the **`worktrees` database table** is
  legacy-internal naming, one row per repo checkout in a workspace
  (`database.rs`). Renaming the table is churn we haven't taken on; it does not
  mean agents use git worktrees.

The historical spelling `~/.fletch/worktrees/` for the on-disk root was renamed
to `~/.fletch/workspaces/`; a one-time migration handles existing installs
(`workspace.rs`).

## Authoritative code

The code is canonical. When this document and the code disagree, the code wins —
and this document should be corrected.

- `crates/fletch-core/src/sandbox/seatbelt.rs` — the macOS `sandbox-exec` profile: the
  deny-writes-then-re-allow model, the writable set, and the app-data deny.
- `crates/fletch-core/src/sandbox/boxes/` — the box engine: the micro-VM per
  agent, the checkout inside the guest, and the guest-home mechanism
  (`home.rs`) that seeds a provider's home and keeps its credential files in
  step.
- `crates/fletch-core/src/sandbox/provider/` — provider knowledge shared by
  every engine: what each CLI keeps outside the checkout, how its credential
  is resolved on the host for a launch (`auth.rs`), and which engines can run
  it (`supports`).
- `crates/fletch-core/src/sandbox/guarantees.rs` — the claim/coverage matrix: what each
  engine actually guarantees, and the stated reason wherever coverage is less
  than complete. Authoritative for per-engine coverage.
- `crates/fletch-core/src/sandbox/policy.rs` — the engine-independent write policy and its
  invariants, including invariant 3 (git-executable config is never
  agent-writable).
- `crates/fletch-core/src/git/hardening.rs` — the overrides every host-side git invocation
  carries, applied at git's single spawn seam.
- `crates/fletch-core/src/sandbox/provision.rs` — how a workspace comes into existence:
  the two clone strategies, origin rewrite, hook install, and identity seeding.
- `crates/fletch-core/src/rpc/git/` — the host-side broker for `git_push`, `open_pr`,
  and `git_fetch`, including the (currently unprompted) publication ops.
