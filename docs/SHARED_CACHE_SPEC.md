# Shared toolchain cache: one API, two engines

Design spec. No code changes land with this document; it describes a target
state and the steps to reach it. Every path and identifier below was checked
against the tree at the time of writing.

## Goal

One shared-cache API that both engines — macOS seatbelt and the box
(libkrun) — implement, with **zero per-stack path lists**. Adding a new stack
(PHP/Composer, Elixir/Hex, JVM/Gradle, …) must require no code change: the
stack's tools find their caches because `HOME` already points at a layered
tree that holds them, not because someone added a row naming that stack.

## Problem

Three hardcoded per-tool lists exist today, plus a fourth in the box's Run
path:

| List | Location | What it enumerates |
| --- | --- | --- |
| `TOOLCHAIN_CACHE_VARS` | `crates/fletch-core/src/sandbox/toolchain_cache/vars.rs` | `(env var, subdir, keying)` per toolchain, consumed by `toolchain_cache_env` |
| `RUN_TOOLCHAIN_DIRS` | `crates/fletch-core/src/sandbox/seatbelt.rs` | home subdirs the Run profile additionally grants (`.cargo`, `.rustup`, `go`, …) |
| `agent_scratch_dirs` | `crates/fletch-core/src/sandbox/policy.rs` | class-2 host-scratch dirs (`.npm`, `.cache`, `Library/Caches`, …) |
| `RUN_WRITABLE_HOME_DIRS` | `crates/fletch-core/src/sandbox/boxes/run.rs` | the Linux twin of the two lists above |

The asymmetry that motivates the change: the box is already tool-agnostic at
the home level. `packaging/box-image/rootfs/opt/fletch/init` gives the guest a
writable home at the host's own home path (`bind_identity "$FLETCH_BOX_HOME"
/data/home`), so a guest toolchain writing anywhere under `$HOME` simply
works. Seatbelt has no such home: an agent runs with the user's real `HOME`
(`dirs::home_dir()` in `crates/fletch-core/src/agent/spawn.rs`), most of which
is not writable, so every toolchain must be bent into the granted cache root by
the env-var table. That table is the thing that has to be edited per stack, and
toolchains with no cache-only variable (Ruby, JVM, and Rust before the
`cargo_home` overlay) are not served by it at all.

Caches are also keyed per project today (`toolchain_cache_root(home,
project_id)`), so two projects never reuse one crates registry, one npm cache,
or one Playwright Chromium.

## Design

### The API

Reshape the existing `prepare_toolchain_cache` in
`crates/fletch-core/src/sandbox/toolchain_cache/mod.rs` — today
`(home, project_id, cwd, platform) -> ToolchainCache { root, env }` — into the
single entry point both engines call:

```
prepare_toolchain_cache(agent_id, cache_root) -> { home_dir, grants }
```

`home_dir` is the `HOME` the confined child runs with. `grants` are the paths
the caller must make writable: a `(subpath …)` each under seatbelt, a writable
bind/mount each in the box. The env the function returns shrinks to `HOME`
alone. Every launch calls it once, so the grant and the env can never name two
different paths — the invariant the current function already holds.

### The layer pyramid

Shared resources are layered, widest at the bottom:

1. **Global** — shared across every project and every agent: one Chromium /
   Playwright browser install, one Cargo registry and crates cache, one npm
   cache, one pnpm store, one Go module cache, and so on for stacks nobody has
   named yet.
2. **Project** — overrides and additions belonging to one project.
3. **Workspace (per agent)** — the agent's own `HOME` subdir, on top.

An agent's `HOME` is the top of this stack, not a sibling of it:
agent-specific state (shell history, scratch config a turn writes) lands in
the agent's own subdir, while the toolchain caches a tool reaches through
`$HOME/...` resolve down to the project layer and then to the global one, so
every agent of every project reuses the same downloads. Nothing in the layout
names a toolchain: the layers are whole directory trees, and a stack Fletch has
never heard of inherits the behaviour by writing under `$HOME` like all the
others.

Writes default to the layer that owns them. A write that must stay private to
one agent lands in its workspace layer; a write to a path the shared layers
already provide (a package added to a cache) lands in the shared layer so the
next agent finds it. Which of the two a given write takes is a property of the
mount/grant configuration, not of a per-tool table.

### Box

The guest already composes read-only layers with overlayfs — `init` overlays
the erofs image layers onto `/usr`, `/etc` and `/var`, and overlays several
tools disks onto `/opt/fletch/tools`. The same mechanism realizes the pyramid:
the global and project layers become lower (read-only, or shared-writable
where write-through is wanted) layers and the agent's workspace layer the
upper one, mounted at the guest's home path. The change is an extension of the
`bind_identity` block in
`packaging/box-image/rootfs/opt/fletch/init` — where `$FLETCH_BOX_HOME`,
`$FLETCH_BOX_RPC_DIR` and `$FLETCH_BOX_WRITABLE_ROOT` are bound from `/data` —
so the home mount becomes a composed mount rather than a plain bind.

The global and project layers live on the host (one copy for the whole
machine), so they reach the guest as virtiofs shares, the mechanism `init`'s
`mount_share` and `fletch_box::guest::ENV_SHARES` already implement for folder
grants. Note the existing constraint: `mount_share` accepts only paths under
`/Users` or `/Volumes`, which the default cache base (`~/.fletch/…`) satisfies
but `FLETCH_TOOLCHAIN_CACHE_ROOT` can be pointed outside of.

### Seatbelt

macOS has no overlayfs, so the equivalent is host-side layering under the same
shared root, with the agent's `HOME` set to its workspace layer:

1. `prepare_toolchain_cache` creates the agent's `home_dir` under the shared
   cache root and composes the layers beneath it.
2. The launch sets the child's `HOME` to `home_dir` instead of leaving it at
   `dirs::home_dir()`.
3. `build_profile` (`crates/fletch-core/src/sandbox/seatbelt.rs`) adds
   `home_dir` and the shared layers it must write through to its allow
   `file-write*` grants, in place of the per-tool grants it emits today.

Composition without a union filesystem is the implementation's choice, and
both candidates stay stack-agnostic because neither enumerates tool names:
directory-level indirection built at launch by enumerating what the lower
layers actually contain, or a copy-up using APFS `clonefile` when an entry must
become private. Reads are unrestricted sandbox-wide (`allow default`), so only
the write path needs a decision.

## Deletions

- `TOOLCHAIN_CACHE_VARS` and `toolchain_cache_env`
  (`toolchain_cache/vars.rs`), with their tests. `HOME` is the only variable
  that remains. `BoxPlatform` stays: the layers are still platform-keyed.
- `RUN_TOOLCHAIN_DIRS` (`seatbelt.rs`) and its use in `build_run_profile`.
- The per-tool entries in `agent_scratch_dirs` (`policy.rs`). The function
  keeps whatever is genuinely not cache (`/private/tmp` class paths stay where
  they are).
- `RUN_WRITABLE_HOME_DIRS` (`boxes/run.rs`), the box's copy of the same two
  lists.
- The Cargo overlay (`crates/fletch-core/src/sandbox/cargo_home.rs`). Cargo's
  home is `$HOME/.cargo` under the new `HOME` with no redirect at all; the one
  piece worth keeping is `cargo_home::prepare`'s credentials symlink
  (`credentials.toml`, `credentials` linked in from the user's real home,
  never `config.toml` — see that module's doc for why the config must not be
  linked), re-homed under the new `HOME` if private-registry auth still has to
  reach the agent.

## Accepted limits

- **Tools that hardcode absolute paths outside `HOME` will not share.** That
  is a property of the tool, documented here, not a missing row somebody
  forgot to add.
- **Isolation granularity differs by engine.** Seatbelt isolation is
  directory-level: each agent gets its own subdir and the kernel enforces
  write grants on it. The box's is VM-level. This asymmetry already exists and
  is not made worse.
- **The project-keyed trust domain is relaxed by design.** The current layout
  is keyed per project precisely so that one project's agents cannot plant
  code that another project's sandboxed build executes (see the module doc of
  `sandbox/toolchain_cache/mod.rs`: cargo compiles `registry/src`, `npx` runs
  what `_npx/` holds, Playwright launches the browser it downloaded). A global
  layer is cross-project by definition, so that argument no longer holds for
  what lives in it. The mitigation is the write path: what the global layer
  accepts, and from whom, is the security-relevant decision of the
  implementation. The host still runs nothing from these trees.
- **The Run panel adopts the same `HOME`.** It gets the same layered home as
  the project's agents rather than its own grant list, which is what lets its
  per-tool list go away. Run remains the documented weaker boundary.

## Migration plan

1. Implement `prepare_toolchain_cache(agent_id, cache_root)` and the layer
   composition, with no caller switched over.
2. Wire both launch paths to it: the seatbelt launch and the box launch
   (`boxes::guest_toolchain_cache` and the `init` mounts).
3. Switch the child env to `HOME`, keeping the old table emitted alongside it
   for one step so a regression is bisectable.
4. Delete the tables listed under *Deletions*, with their tests.
5. Add a test that launches a fictional tool which only reads and writes
   `$HOME/.config/fictional-tool`, asserts the file lands on the shared
   volume rather than in the agent's private layer, and asserts a **second
   agent sees it**. Because the tool is fictional, the test fails if any
   per-tool table is reintroduced, and passing it is the definition of
   "a new stack needs no code change".
6. Update the docs that describe the old mechanism: `docs/isolation.md` (the
   seatbelt-allows section and the toolchain-cache section), `docs/box.md`
   (the guest-cache and Run-view sections) and `docs/sandbox-foundation.md`,
   plus the module doc of `sandbox/toolchain_cache/mod.rs`.

## Why

One code path instead of four lists. Today, supporting a new stack means
editing `TOOLCHAIN_CACHE_VARS`, possibly `RUN_TOOLCHAIN_DIRS`,
`agent_scratch_dirs` and `RUN_WRITABLE_HOME_DIRS`, keeping the seatbelt and box
copies in sync, and discovering afterwards whether the stack even has a
cache-only env var. Afterwards it means nothing: the stack writes under `$HOME`
and the layers carry it.

## Open questions

1. **What host-home state must stay reachable from the new `HOME`?** A
   seatbelt agent today runs with the user's real home and reads real state
   from it: the provider CLIs' config and auth (`~/.claude` plus
   `~/.claude.json` and the claude credentials file, `~/.cursor`, `~/.gemini`,
   `~/.pi`, opencode's XDG data and config dirs), and the user's global
   gitconfig, which the seatbelt engine lets git read where a box cannot.
   Repointing `HOME` moves every one of those lookups. Git is the least
   affected: the identity and `commit.gpgsign=false` are already frozen into
   each checkout's own config (`sandbox::provision::seed_identity`) precisely
   so a sandbox that cannot see the global file still commits, which makes a
   seatbelt agent behave like a box agent here rather than breaking.
   Deliberately *not* reachable, and which must stay that way: `~/.ssh`, the
   `gh` CLI login, and the git credential store, all denied by name in
   `seatbelt.rs`. The spec needs a decision per item — linked into the new
   home, copied per launch, or left behind — and the provider-state ones
   interact with the host-side readers that resolve the same paths from the
   host env.
2. **What happens to the provider state dirs granted in `policy.rs`?**
   `provider_state_dirs` / `all_provider_state_dirs` are class-1
   host-persistence grants: that state must land on the *host* to survive the
   process and be read back. They are home-relative, so they are affected by
   the `HOME` move but are not caches and do not belong in a shared layer.
   Codex is the precedent for the other direction: it gets a per-agent
   `CODEX_HOME` overlay under the writable root and no grant at all. The
   question is whether provider state stays anchored to the real home
   independently of `HOME`, or moves into the agent's workspace layer with a
   host-side sync.
