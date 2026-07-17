# Multi-repo projects — follow-up work

Hand-off document for the work remaining after the multi-repo stack:

- **PR #458** `feat/multi-repo-projects` — projects hold N repos; Repositories
  field in project settings (attach/detach/relocate, per-repo labels); sidebar
  groups by project.
- **PR #459** `feat/multi-repo-spawn` — spawn forks a checkout of every project
  repo; agents get a workspace-layout note; host git RPC ops accept `args.repo`;
  branch/PR events carry the targeted repo.
- **PR #460** `feat/multi-repo-git-panel` — panel commands take an optional
  `subdir`; Git panel renders per-repo sections with progressive disclosure;
  delegations are repo-scoped.

Read this primer first; each task below is self-contained after it.

## Architecture primer (state as of PR #460)

**Data model.** A project is `projects(id, name)` — no path. Repos hang off it:
`repos(id, project_id, path UNIQUE, label)`. An agent (a `workspaces` row) has
one `worktrees` row per checkout: `worktrees(workspace_id, repo_id, subdir,
branch, parent_branch, base_sha, pr_number, pr_url, pr_title, pr_state, …)`.
In memory that's `AgentRecord.repos: Vec<TrackedRepo>` — always non-empty,
`repos[0]` is the **primary** (the repo the user spawned against). A repo
within an agent is addressed by its `subdir` — the checkout's directory name
under `~/.fletch/workspaces/<agent-id>/`. `TrackedRepo.label` is denormalized
from `repos.label` at query time (`query_tracked_repos`,
`src-tauri/src/workspace.rs`).

**The primary-default convention.** Every per-agent surface takes an optional
repo scope and defaults to the primary, so single-repo behavior is
byte-identical everywhere:

- Backend panel commands: `subdir: Option<String>` resolved by
  `agent_repo_checkout(_opt)` (`src-tauri/src/commands.rs`, near the other
  `fn primary_*` helpers).
- PR resolution: `resolve_pr_state(workspace, agent_id, subdir: Option<&str>)`
  (`src-tauri/src/supervisor/session_sync.rs`).
- Agent-facing RPC (`git_push` / `open_pr` / `git_fetch` / `git_status`):
  optional `args.repo` = subdir (`src-tauri/src/rpc/git.rs`, `GitDispatcher`,
  built per-spawn in `supervisor/lifecycle.rs::start_process`). Emitted
  `git.branch_created` / `git.pr_opened` events carry `repo`; the consumer
  (`supervisor/rpc_watch.rs`) persists branch/PR onto the matching worktree
  row, falling back to primary when the field is absent.
- Frontend store: `gitKey(agentId, subdir?)` in `src/store/git.ts` —
  `subdir ? `${agentId}::${subdir}` : agentId`. The **plain agent id is the
  primary's key** and is the only key the app-wide write paths use (bulk
  polls, `pr:state_changed` reducer in `src/store/app.ts`, sidebar badges,
  `src/util/prState.ts`). Only the Git panel's per-repo fetches use suffixed
  keys.
- Git panel: `GitPanel` (`src/components/RightPanel/GitPanel/index.tsx`)
  orchestrates `GitRepoSection` per repo. A repo renders a section when
  "active" (changed files ∨ branch ∨ PR ∨ pinned by an in-flight delegation).
  Single-repo agents early-return the bare section — pixel-identical to the
  old panel. Delegations (`GitDelegation` in
  `src/components/RightPanel/delegation.ts`) carry `subdir?`; the matching
  section's `useDelegationLifecycle` owns the lifecycle.

**Non-negotiable invariants for all follow-ups:**

1. Single-repo projects/agents must stay byte-identical (no visual or
   behavioral diff). The pattern that achieves this everywhere: optional scope
   param, primary default, plain-agent-id store keys for the primary.
2. Never attribute one repo's git/PR state to another. When adding a repo
   dimension to a currently agent-keyed path, either thread the subdir or
   leave the path primary-only — no silent `repos[0]` when a scope exists.
3. Workflow step agents are single-repo by design (`wf_run.repo_path`); don't
   accidentally multi-repo them (spawn skips secondaries when
   `run_repo_for_task.is_some()` — keep that).

Verification baseline: `cargo test --lib` (src-tauri; ~890 tests),
`bun run check`, `bun run lint` (0 errors / ~72 legacy warnings),
`bun run test` (447). In sandboxed environments set `TMPDIR=$PWD/.tmp` for bun.

---

## 1. Sidebar PR badge fan-out  ·  priority: high  ·  size: M

**Problem.** The app-wide PR polls and every badge read only the primary:
`refresh_all_pr_states` / `refresh_all_pr_checks`
(`src-tauri/src/commands.rs`) and `resolve_all_pr_states`
(`supervisor/session_sync.rs`) iterate `agent.repos.first()`; the frontend
maps `prStates[agentId]` / `prChecks[agentId]` hold one entry per agent;
`src/util/prState.ts` falls back to `repos[0]`. A multi-repo agent whose only
PR is on a secondary repo shows no sidebar badge at all (the panel is
correct; the badge isn't).

**Sketch.**
- Backend: change the pending-set builders in `resolve_all_pr_states` and
  `refresh_all_pr_checks` to iterate all `agent.repos` (each entry already
  tracks `subdir`; `Pending { agent_id, subdir, … }` exists). Return maps
  keyed `agent_id -> Vec<{subdir, state}>` or flat `"{agent_id}::{subdir}"`
  string keys — pick ONE and mirror `gitKey` exactly (`::` separator; primary
  entry keyed by plain agent id to preserve every existing consumer).
  Preserve the omit-on-unknown contract (absent ≠ null) per the doc comments
  on both commands.
- Frontend: the poll reducers (`refreshAllPrStates` / `refreshAllPrChecks` in
  `src/store/git.ts`) merge the returned keys as-is — if the backend keys by
  `gitKey`, no reducer change is needed beyond accepting extra keys.
- Badge UI (`src/components/Sidebar/AgentRow.tsx` + `src/util/prState.ts`):
  aggregate across the agent's repos. Suggested rendering: keep the single PR
  pill when exactly one repo has a PR (whatever repo it's on — this fixes the
  main bug); with >1, show `N PRs` with the *worst* status tint (any failing
  checks → failing; any open → open; all merged → merged). Keep
  `prState.ts`'s snapshot-fallback policy per repo (`prSnapshot(repo)` works
  on any TrackedRepo).

**Acceptance.**
- Agent with PR only on secondary repo → sidebar badge shows it.
- Two PRs → aggregate pill; tint follows worst status; opening the agent
  shows the matching panel sections.
- Single-repo agents: unchanged badge, unchanged poll payload size (one entry).
- No per-agent `gh` fan-out: the batched GraphQL query grows by repo count,
  not by extra round trips (both batch fns already alias per-lookup).

**Pitfalls.** The one-PR-per-agent shape is asserted in comments
(`commands.rs` "multi-repo PR tracking is out of scope here" — delete those);
`session_sync.rs` has `pr_opened_at/pr_merged_at` timestamps persisted per
worktree via `persist_pr_snapshot` — already per-repo, don't duplicate.
Delegation attribution (`markGitDelegationSawOp` path) keys off
`agent:git-action` events, not prStates — untouched.

## 2. "One task → N PRs" presented as a unit  ·  priority: high  ·  size: M

**Problem.** Each panel section shows its own PR, but nothing communicates
"this task produced 3 related PRs": no cross-links in PR bodies, no suggested
merge order, no combined status.

**Sketch.**
- Cross-linking at creation: when `open_pr` (RPC, `src-tauri/src/rpc/git.rs`)
  or `create_pr` (panel command) runs for an agent that already has other
  bound PRs, append a generated trailer to the body:
  `---\nPart of a multi-repo change in <project name>: fwdai/frontend#12,
  fwdai/backend#34`. Backfill the older PRs' bodies via a
  best-effort `gh pr edit`-equivalent (there's a GitHub client in
  `src-tauri/src/github/mod.rs`; add `pr_update_body(checkout, number, body)`
  if missing). Idempotency: mark the trailer with an HTML comment sentinel
  (`<!-- fletch:pr-set -->`) and replace between sentinels rather than append.
- Panel summary strip: in `MultiRepoGitPanel`, when ≥2 sections have PRs,
  render a slim strip above the sections: `2 PRs · 1 green · 1 checks running`
  with each PR number linking out. Data is already in the store under
  `gitKey` keys.
- Merge order: keep it lightweight — a static hint derived from repo order
  (primary last?) is guesswork; better to let the user merge from each
  section and skip ordering logic entirely in v1. Do NOT build cross-repo
  merge gating yet.

**Acceptance.** Opening 2nd PR of an agent updates both bodies with the set
trailer exactly once (re-running doesn't duplicate); panel shows the summary
strip; single-PR agents see no trailer and no strip.

**Pitfalls.** Body edits race the agent's own `open_pr` body — sentinel
replacement, never string-append. `pr_create` returns `PrState` without body;
fetch before edit. Respect the rate-limit backoff guard
(`gh::client::is_backing_off()`).

## 3. Code tab (file tree + editor) multi-repo  ·  priority: medium  ·  size: M

**Problem.** `list_checkout_tree`, `read/write/create/copy_checkout_file`
(`src-tauri/src/commands.rs`, helper `primary_checkout`) operate on the
primary checkout only. A multi-repo agent's secondary-repo edits are
invisible in the Code tab (`src/components/RightPanel/FilePanel/index.tsx`).

**Sketch (choose A; B listed for completeness).**
- **A — repo-prefixed virtual root (recommended):** for `repos.len() > 1`,
  `list_checkout_tree` returns paths prefixed `"<subdir>/…"` (walk each
  checkout, prefix, concat; per-repo git status vs each repo's own
  `diff_base`). File read/write commands split the first path segment against
  the agent's subdirs to pick the checkout, falling back to primary-relative
  for single-repo (exact current behavior). Frontend FilePanel needs little:
  the tree component already nests by `/`; top-level folders become the repos.
  Diff view: per-file diff fetch must use the same subdir resolution.
- **B — repo selector dropdown in the panel:** smaller backend change
  (`subdir` param like the git commands) but adds a UI mode switch; rejected
  by the product direction (obscure the plumbing; one tree).

**Acceptance.** Multi-repo agent: tree shows one top-level folder per repo
with per-repo status badges; opening/editing/creating a file in a secondary
repo works; single-repo agents see today's un-prefixed tree.

**Pitfalls.** Path-splitting must not treat a real top-level directory named
like a subdir in the *single*-repo case (gate all prefix logic on
`repos.len() > 1`). `write_checkout_file` safety checks (path traversal
guards) must run against the resolved checkout root.

## 4. Per-repo base branch at spawn  ·  priority: low  ·  size: S

**Problem.** The new-agent BranchPicker sets the base for the primary only;
secondaries fork from their repo's current branch
(`attach_repo_checkout` in `supervisor/lifecycle.rs` uses
`git::current_branch(&repo_path)`).

**Sketch.** Add a nullable `default_base` column to `repos` (migration 0023,
mirror 0022_repo_label.sql), editable in the Repositories field rows
(`src/components/ProjectSettings/RepositoriesField.tsx`) as a small branch
input/picker. `attach_repo_checkout` prefers `default_base` when set. The
spawn-time picker continues to govern the primary.

**Acceptance.** Set backend default base = `develop` → new agents' backend
checkout forks from `develop` (verify `parent_branch` on the worktree row);
unset → current behavior.

## 5. Secondary push → PR state latency  ·  priority: low  ·  size: S

**Problem.** After a push on a secondary repo, `push_agent` fires
`fetch_and_emit_pr_state(app, agent_id)` (primary-scoped) and the
`pr:state_changed` event is agent-keyed, so the secondary section's PR card
updates only on its 5s poll.

**Sketch.** Thread subdir: `fetch_and_emit_pr_state(app, agent_id, subdir)`
(`session_sync.rs` — it already calls `resolve_pr_state(…, None)`; pass the
subdir through) and extend the `pr:state_changed` payload with optional
`subdir`. Frontend reducer (`src/store/app.ts`, search `pr:state_changed`)
writes under `gitKey(agent_id, subdir)`. RPC-side `EVENT_PR_OPENED` handling
in `rpc_watch.rs` calls `fetch_and_emit_pr_state` — pass the event's repo.
Old events without the field keep the plain key (primary) — same
compatibility dance as the branch events.

**Acceptance.** Agent opens PR on secondary via RPC → that section's card
flips to "PR open" within ~1s (event path), not 5s (poll).

## 6. `publish_agent` repo-awareness  ·  priority: low  ·  size: S

**Problem.** "Publish to GitHub" (local-only repo, no origin) publishes the
primary (`commands.rs::publish_agent`). A secondary section with
`has_origin: false` shows the publish action but would publish the wrong
repo.

**Sketch.** Add `subdir: Option<String>` like the other panel commands; the
frontend `useGitActions` "publish" case passes `ctx.subdir` (it already has
it). Check the backend impl for primary assumptions (it publishes from the
*source repo root*, not the checkout — resolve the source repo through the
agent's TrackedRepo by subdir).

**Acceptance.** Local-only secondary repo publishes itself; primary flow
unchanged.

## 7. Multi-repo workflows  ·  priority: parked  ·  size: L — design first

`wf_run` has `repo_path` + `base_branch` baked into schema (migration 0019 /
0020) and every step provisions from that one repo
(`workflow/scheduler.rs::wf_launch`, `provision_forking_run_repo` call sites).
Options when demand appears: (a) run-level repo set with per-step primary,
(b) per-step repo scope in workflow YAML (`src/workflows/spec.ts` `Step`),
(c) keep runs single-repo and compose one run per repo. Don't start without a
design doc; the scheduler's step hand-off (refs `refs/wf/steps/<prev>`) is
single-repo throughout, and finalize (`Finalize { push, open_pr, pr_base }`)
assumes one PR target. Free-form agents already cover multi-repo tasks — this
is the deliberate v1 boundary.

## 8. Merging projects that have history  ·  priority: parked  ·  size: M

`attach_repo_to_project` (`workspace.rs`) moves a repo out of its old project
only when that project is empty; a project with agents/workflow runs is
rejected with an explanatory error. Lifting that means re-pointing
`workspaces.project_id` and `wf_run.project_id` for the moved repo's agents —
but agents can have checkouts of *multiple* repos, so "which project owns the
agent" becomes ambiguous the moment projects merge. Needs a decision (likely:
move all agents whose primary repo is the moved repo; refuse if any agent
spans both projects), plus extending `AttachOutcome`/`undo_attach` to cover
the re-pointing rollback. Don't attempt as a drive-by.

---

## Suggested sequencing

1 + 2 together ("multi-repo review surface" — they share the PR fan-out
plumbing), then 3, then 5 + 6 as a small cleanup pair, 4 when someone asks,
7/8 on demand with design first. Base each PR on the multi-repo stack (or
main once #458–#460 land).
