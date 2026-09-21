# Multi-host: closing the four remaining gaps

2026-09-21. Each gap from the audit was re-checked against the code as merged
(#749–#784). Verdicts first, then one PR per line of work with scope, files,
acceptance and order. Nothing here changes the local desktop path.

Status (2026-09-21): PR 5 → #785, PR 1 → #786, PR 2 → #788, PR 3 → #787,
all merged. PR 4 → #789 and PR 6 → #791 open, review threads addressed. The
manual checks for all of it are in `multi-host-test-plan.md`.

## 1. Verification

### Gap 1 — no provider CLI login on a headless host: **verified**

- `crates/fletch-host/src/login.rs` is GitHub-only; the admin socket answers
  `github_login_start` / `github_login_status` and nothing provider-related
  (`crates/fletch-host/src/ops.rs:45-96`).
- `src-tauri/src/commands/provider_login.rs:35-108` spawns the pinned login
  command in a PTY and streams `provider-login:output/exit` to the webview.
  The pinned table lives in `src-tauri/src/provider_login.rs:28-36`:
  claude `auth login`, codex `login`, cursor-agent `login`, opencode `auth
  login`; antigravity and pi have no command.
- What is already Tauri-free in `fletch-core`: `PtySession`, `resolve_agent_bin`
  (honours the bin override), `probe_all_providers` (installed + version) and
  `probe_all_provider_auth` (`agent/auth_probe.rs:63-96`, checks
  `~/.claude/.credentials.json`, `~/.codex/auth.json`, env keys, keychain).
- No provider op is on the wire (`remote/dispatch.rs:110-217`). The
  `never_exposed_ops_are_not_dispatchable` test names only `install_agent`,
  not `open_provider_login` / `probe_provider_*`.
- Two precisions. There is no `gemini` provider (the README's prose is loose;
  the set is claude, codex, cursor, antigravity, opencode, pi). And in
  paired-host mode the desktop's Settings › Providers is `invokeLocal`
  throughout (`src/api/domains/providers.ts:13-58`), so it shows and signs in
  **this Mac's** CLIs while a remote host is active, with no hint. The model
  picker is built from this Mac's catalog too.

### Gap 2 — projects only where someone has local access: **verified, two corrections**

- `GATES.addProject` is `op: null` (`src/store/capabilities.ts:38-42`), so it
  closes on every remote environment. `list_dir`, `add_workspace_repo`,
  `clone_repo`, `gh_status`, `gh_repo_list` are on the wire. `api.listDir`
  already routes through the active transport (`src/api/domains/files.ts:12`).
- Correction 1: of the 7 `plugin-dialog` files only 4 feed project ops
  (`NewProjectPopover`, `Home`, `useNewProject`, `RepositoriesField`). The
  other 3 pick client-local files for YAML import/export, feedback screenshots
  and attachments, and are unrelated.
- Correction 2: the gate also covers attach/relocate (its own comment,
  `capabilities.ts:35-37`), and those ops are **not** on the wire. Neither are
  `create_repo`, `rename_project`, `delete_project`, `detach_repo_from_project`,
  `set_repo_label`, `remove_workspace_repo`, `project_has_running_agents`. A
  browser alone opens Open-folder and Clone; it does not open Create or the
  project settings page.
- The mobile app already ships the browser: `mobile/src/sheets/AddProjectSheet/FolderPicker.tsx`
  (dirs only, hidden toggle, Up, anchored on the host's `listing.base`) plus
  per-host remembered destination (`persist.ts:21-24`). Its logic is ~50 lines
  of state inside the component, trivially extractable into a hook. Desktop's
  remembered parent is one `localStorage` key, not per host.
- `list_dir` returns files and hidden entries, unsorted, capped at 1000, and
  does not say whether a directory is a git repo.

### Gap 3 — no scope at pairing: **verified**

- `DeviceRecord` (`remote/auth.rs:107-134`) has no authorization field. The
  pairing token binds nothing but itself and an expiry (`auth.rs:40-49`).
- The single gate is `dispatch::is_allowed(op)` at `remote/server.rs:522-530`,
  after `authenticated` is set. `SupervisorDispatch::dispatch` carries no
  identity on purpose, so the check belongs in `server.rs`.
- `protocol_descriptor()` (`remote/mod.rs:641-648`) is global; both `pair`
  and `hello` return the same list.
- No op-family table exists; `OPS` is one flat list. Adding a field is safe:
  `parse_devices` drops records that fail to deserialize, so the field must
  carry a `serde(default)`, exactly like `push_token` does.

### Gap 4 — plaintext secrets on a headless host: **verified, and already documented**

- `secrets::use_settings_store_only()` is called only when headless
  (`host/boot.rs:248-254`). Three keys go through it: `github_token`,
  `linear_token`, `claude_container_token`.
- The README already has the section the audit asks for: "The data dir, and
  who may read it" (`crates/fletch-host/README.md:315-380`) states the token is
  plaintext in the settings table, why, the 0700 dir and 0600 socket, and "run
  the host as its own user". `serve.rs:197-215` re-restricts the dir to 0700
  on every start.
- What is missing: no env or systemd-credential path. The seam is one line:
  the GitHub token is read once at boot by `seed_secret_mirror` (`boot.rs:435`)
  into `github::client::token()`, the sole read path for git and the API.
  `packaging/fletch-host.service` has no `LoadCredential=`. The README's
  "Known gaps" still says there is no `fletch-host update`.

## 2. Plan

Six PRs. Order and parallelism at the end. Every PR keeps the local desktop
byte-for-byte in behaviour, passes the usual gates (`cargo fmt/clippy/test`
for the crates, `bun run lint|check|build|test` root and `mobile/`), and
updates `docs/remote-protocol.md` before code when it touches the wire.

### PR 1 — Remote add-project: host folder browser (gap 2, part a)

**UX.** On a remote environment the "+" button and the Home card work as they
do locally. "Open a folder" and the Clone destination open a modal that
browses the host's disk: a path field you can type or paste into (Enter jumps
there), "Up", a directory list (dirs only, sorted, hidden toggle), git repos
marked with a badge, and a primary button "Use this folder" showing the
resolved absolute path. It starts at the last folder used on that host, else
`~`. Errors (permission denied, missing path) show inline and leave the list
on the last good folder. On the local environment nothing changes: the native
picker stays.

**Scope.**
- Extract the browser state from mobile's `FolderPicker.tsx` into a shared
  hook `useFolderBrowser({ listDir, start })` and move `childPath` /
  `parentPath` / `baseName` to a shared module; mobile imports them back
  (it already imports desktop types via `@desktop/…`).
- Desktop `FolderPicker` component (modal, `layer="overlay"` so it stacks over
  the NewProject modal) built on the hook.
- `pickFolder(env)` helper: local → `open({ directory: true })`, remote → the
  modal. Used by `NewProjectPopover` "Open a folder", `Home.addProject` and
  `useNewProject.pickParent`.
- Remembered parent keyed per environment (`q2:newProjectParent` stays for
  local; `…:<hostKey>` for hosts), matching mobile's `destParents`.
- `GATES.addProject.op = "add_workspace_repo"`. New gate `createProject` with
  `op: "create_repo"` (closed on every current host until PR 2); the popover
  hides "Create new project" with the reason. `hostSkew` now counts both.
- Host: `list_dir` gains an additive `is_repo` per entry (`.git` exists). No
  protocol bump; doc row updated.

**Files.** `src/store/capabilities.ts` (+test), `src/components/NewProject/{useNewProject.ts,index.tsx,shared.tsx,CloneView.tsx}`,
`src/components/Sidebar/{NewProjectPopover.tsx,index.tsx}`,
`src/components/Workspace/Home/index.tsx`, new `src/components/FolderPicker/`,
new `src/util/{folderBrowser.ts,paths.ts}` (+tests), `mobile/src/lib/paths.ts`
and `mobile/src/sheets/AddProjectSheet/FolderPicker.tsx` (consume the shared
code), `crates/fletch-core/src/commands/files.rs` (+test),
`docs/remote-protocol.md`.

**Acceptance.** Remote env active: "+" → Open a folder → browser at `~` →
pick a repo → project appears on the host and in the client without a
refetch. Clone from GitHub picks its destination in the browser and the
remembered folder is per host. "Create new project" hidden with reason.
Local env: native picker, no modal, no behaviour change. Mobile tests green
on the shared hook.

### PR 2 — Project settings on the wire (gap 2, part b)

**UX.** The project settings page works for a remote host: rename, delete,
attach/detach/relocate a repo (attach and relocate use PR 1's browser), set a
label, and "Create new project" in the popover. On an older host each action
is disabled with a reason, like the Git panel.

**Scope.**
- `OPS` rows, dispatcher arms, protocol doc rows and parity tests for
  `create_repo`, `rename_project`, `delete_project`, `remove_workspace_repo`,
  `attach_repo_to_project`, `detach_repo_from_project`, `set_repo_label`,
  `relocate_repo`, `project_has_running_agents`. Move the two-phase attach
  helper from `src-tauri/src/commands/workspace.rs:65-81` into
  `crates/fletch-core/src/commands/workspace.rs` so both fronts share it.
- Client: `RepositoriesField` uses `pickFolder(env)`; two gates,
  `projectAdmin` (opened by `rename_project`) and `createProject` from PR 1;
  `DeleteSection` and the field's buttons disabled with reason when closed.

**Files.** `crates/fletch-core/src/remote/{dispatch.rs,tests.rs}`,
`crates/fletch-core/src/commands/workspace.rs`, `src-tauri/src/commands/workspace.rs`,
`src/store/capabilities.ts`, `src/components/ProjectScreen/{RepositoriesField.tsx,DeleteSection.tsx,...}`,
`src/components/NewProject/CreateView.tsx`, `docs/remote-protocol.md`.

**Acceptance.** Every project settings action works against a host built
from this PR and shows a reason against a host built from PR 1. Local
unchanged. `the_protocol_descriptor_reports_the_whole_wire_surface` updated.

### PR 3 — `fletch-host provider status | login <id>` (gap 1, part a)

**UX.** On the host, over SSH as the service user:

```
$ fletch-host provider status
provider    installed   auth         fix
claude      2.1.4       signed out   fletch-host provider login claude
codex       0.48.0      signed in
cursor      —           not found    https://cursor.com/install
antigravity —           not found    sign in on a machine with a browser, copy ~/.gemini/antigravity-cli
$ fletch-host provider login claude
```

`login` runs the same pinned command the desktop runs, resolved the same way
(bin override, login-shell PATH, portable git), **in the operator's own
terminal** with inherited stdio. No PTY plumbing, real TTY, colours and
resizing for free. When it exits the CLI re-probes and prints the new state.
Providers without a command (antigravity, pi) print what to copy where.

**Scope.**
- Move the pinned table to `crates/fletch-core/src/agent/login.rs`
  (`login_command(id) -> Option<&'static [&'static str]>`); the desktop's
  `provider_login.rs` calls it. Pin headless-friendly variants only where the
  installed CLI supports them; verify `codex login --device-auth` against the
  current Codex release before pinning, and keep claude's default (it prints a
  URL and accepts the pasted code).
- Admin socket ops `provider_status` (installed, version, auth, detail,
  loginCommand per provider; from `probe_all_providers` +
  `probe_all_provider_auth`) and `provider_login_command { id }` (resolved
  argv + env). `status` JSON gains a `providers` summary.
- CLI subcommands; a warning when the invoking uid is not the data dir's
  owner (credentials would land in the wrong home).
- README: replace "ssh in and run claude login" with the commands; fix
  "gemini" and the stale "No `fletch-host update`" line.
- Add `open_provider_login`, `probe_provider_auth`, `probe_provider_versions`
  to `never_exposed_ops_are_not_dispatchable` so the wire stays closed.

**Files.** new `crates/fletch-core/src/agent/login.rs` (+tests),
`src-tauri/src/provider_login.rs`, `crates/fletch-host/src/{main.rs,ops.rs,provider.rs (new)}`
(+tests), `crates/fletch-host/README.md`, `crates/fletch-core/src/remote/tests.rs`.

**Acceptance.** On a headless Mac: `provider status` shows claude signed out;
`provider login claude` completes in the terminal; status flips to signed in;
an agent spawned from the phone runs under those credentials. Desktop sign-in
flow unchanged (same table, same commands).

### PR 4 — Host provider state visible to clients (gap 1, part b)

**UX.** A client never spawns onto a host that cannot run the provider. The
spawn flow's provider picker disables what the active host lacks: "Not
installed on mini" or "Not signed in on mini · run `fletch-host provider login
claude` there". The paired-host row and the switcher entry show the same
lines. Settings › Providers gets a one-line notice while a remote env is
active: "These are this Mac's providers. mini's are managed on mini."

**Scope.**
- `probe_provider_versions` and `probe_provider_auth` on the wire as
  read-only ops (check `detail` strings carry no paths or secrets; strip if
  they do). Refetched on env switch, reconnect and when the spawn sheet opens.
- Environment entry gains `providers`; picker, host row, switcher and the
  Settings notice consume it. Local env: unchanged, no notice.
- Mobile: same gating in the new-agent sheet (small; can be its own follow-up
  if the sheet's model picker makes it awkward).

**Files.** `crates/fletch-core/src/remote/{dispatch.rs,tests.rs}`,
`docs/remote-protocol.md`, `src/store/{environments.ts,capabilities.ts}`,
`src/remote/registry.ts`, the spawn provider picker under `src/components/Workspace/EmptyWorkspace*`,
`src/components/SettingsScreen/RemoteControl/HostRow.tsx`,
`src/components/Sidebar/EnvironmentSwitcher/EnvironmentMenu.tsx`,
`src/components/SettingsScreen/ProvidersPane/index.tsx`, mobile new-agent sheet.

**Acceptance.** Switch to a host with claude signed out: picker shows the
reason; sign in on the host (PR 3); reconnect or reopen the sheet: enabled.

### PR 5 — Secrets from the environment on a headless host (gap 4)

**UX.** A cloud host can keep the GitHub token out of its database:

```
FLETCH_GITHUB_TOKEN=ghp_… fletch-host serve          # or
LoadCredential=github_token:/etc/fletch/github_token  # in the unit
```

`status` reports `githubTokenSource: "env" | "credential" | "store" | null`.
`github login` while an env token is active warns that the env value wins on
the next start.

**Scope.**
- In `boot`, when headless, read `FLETCH_GITHUB_TOKEN` or
  `$CREDENTIALS_DIRECTORY/github_token` ahead of `seed_secret_mirror`; seed
  the mirror, never persist.
- `packaging/fletch-host.service`: commented `LoadCredential=` line and the
  matching comment in the plist's `EnvironmentVariables`.
- README: subsection "Keeping the token out of the database" (env,
  credential, encrypted volume advice); fix "Known gaps".

**Files.** `crates/fletch-core/src/host/boot.rs` (+test),
`crates/fletch-host/src/{ops.rs,login.rs}`, `packaging/fletch-host.service`,
`packaging/com.fletch.host.plist`, `crates/fletch-host/README.md`.

**Acceptance.** With the env var set: pushes and PRs work and the settings
table has no `github_token` row; without it, behaviour as today.

### PR 6 — Device scopes at pairing (gap 3)

**UX.** Pairing offers a preset before the code is generated:

- **Full** (default, today's surface): everything.
- **Control**: watch and steer agents, approve tool use, add projects, drive
  workflows and the roadmap, but no `push_agent`, `create_pr`, `merge_pr`,
  `roadmap_merge_item_pr` or `answer_publish_approval`.

The devices list shows the preset next to each device. A device outside its
scope gets `forbidden`, and because `protocol.ops` is filtered per device the
phone and the desktop client hide those actions with a reason, with no client
change. `fletch-host pair --scope full|control` and `devices list` follow.
Changing a device's scope is re-pair; editing in place is a follow-up.

**Scope.**
- `dispatch.rs`: `scope_of(op) -> Scope` over a small fixed set (`observe`,
  `agents`, `projects`, `workflows`, `roadmap`, `publish`) with a test that
  every `OPS` row maps to exactly one scope (PRs 2 and 4 add their rows to it
  if they land first; the test enforces).
- `DeviceRecord.scopes: Vec<String>` with `serde(default = all)`;
  `Pending`/`MintedToken` carry the scope set; `begin_pairing(preset)`;
  `register(.., scopes)`; re-pairing on the same key takes the new token's
  scopes.
- `server.rs`: scope check right after `is_allowed`, new reserved error
  `forbidden`; `protocol_descriptor(scopes)` filters `ops` in both `pair` and
  `hello`.
- Desktop: preset control in the "Pair a device" row, preset column in
  `DeviceRow`; `remote_begin_pairing(preset)`. CLI flag. Protocol doc:
  "Scopes" section, `forbidden`, per-device `ops`.
- Mobile copy: the approval card's fallback line becomes "This device can't
  answer approvals" when the op is absent, instead of "too old".

**Files.** `crates/fletch-core/src/remote/{auth.rs,dispatch.rs,mod.rs,server.rs,tests.rs}`,
`src-tauri/src/commands/remote.rs`, `src/api/{domains/remote.ts,types/remote.ts}`,
`src/components/SettingsScreen/RemoteControl/{index.tsx,useRemote.ts,PairingCard.tsx,DeviceRow.tsx}`,
`crates/fletch-host/src/{main.rs,ops.rs}`, `mobile/src/screens/Agent/ApprovalCard.tsx`,
`docs/remote-protocol.md`.

**Acceptance.** Existing `devices.json` loads with Full. Pair a phone as
Control: `push_agent` → `forbidden`; the phone's Push and Approve buttons are
absent; a desktop client paired as Control shows Push disabled with the
reason. A Full device behaves exactly as today.

## 3. Order

| Wave | PRs | Why |
|---|---|---|
| 1 | PR 1, PR 3, PR 5 | Independent; the two "crucial" gaps start here, PR 5 is small |
| 2 | PR 2, PR 4 | PR 2 needs PR 1's `pickFolder`; PR 4 needs PR 3's login table and status shape |
| 3 | PR 6 | Last so its scope table covers every op the earlier PRs add |

Wave 1 can run as three parallel worktrees. Each PR is independently
releasable; a host and client from different waves stay compatible because
everything is additive and gated by op membership.

## 4. Deliberately not in this plan

- A client-driven provider login (typing into the host's login PTY from the
  phone or desktop). It needs the PTY-stream work (plan §5.3 item 14) or a
  device-code parser per CLI; PR 3 + PR 4 cover the cloud-box story without it.
- In-place scope editing, a `terminal`/`files:write` scope and the pairing
  second factor. They arrive with the deferred terminal work.
- systemd hardening directives (`ProtectSystem`, `PrivateTmp`) in the unit:
  they interact with Docker socket access and deserve their own check.
