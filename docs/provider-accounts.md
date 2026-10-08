# Provider accounts: design, state, and the PR 3 brief

Multi-account sign-in for Claude Code and Codex inside Fletch. This file is the
handoff for whoever implements PR 3 (per-account usage and limits). It records
what PRs 1 and 2 built and why, where every seam lives, the decisions Alex has
already made, and exactly what PR 3 has to deliver. Read it before touching code.

## Status (2026-10-07)

| PR | Branch | Scope | State |
|----|--------|-------|-------|
| #874 | `feat/provider-accounts` | accounts model, per-account login, probe, Settings list | CI green, awaiting merge |
| #875 | `feat/provider-accounts-spawn` (stacked on #874) | active account honoured at spawn, both sandbox engines, transcripts | CI green, awaiting merge |
| PR 3 | `feat/provider-accounts-usage` (stacked on #875) | per-account spend + limit meters | implemented; status-line source deferred |
| host login 1 | `feat/claude-host-login` | the host owns every claude login: refresh, token injection on every engine, relaunch-on-expiry, attribution by stamp | draft (2026-10-08) |
| host login 2 | `feat/account-switch` (stacked on host login 1) | mid-session account switch (`switch_agent_account`: restamp, relaunch on the other token via `relaunch_locked`) | draft (2026-10-08) |
| host login 3 | `feat/codex-host-login` (stacked on host login 2) | the host owns every codex login: per-agent `CODEX_HOME` overlay, launch credential without a refresh token, host refresh; codex switching unlocked | draft (2026-10-08) |

Merge #874 first, then #875. Manual checks Alex still owes before merging #875:
a fresh Claude account click-through (add → sign in → make active → new agent)
on both the macOS sandbox and Docker, watching for Claude's onboarding/theme
screen in the agent terminal; and one account removal to confirm the Keychain
delete raises no macOS prompt. The ignored kernel test
`seatbelt_enforces_account_dir_grants` has been run on Alex's Mac and passes.

Deferred on purpose: automatic account switching when a limit is hit. Do not
build it in PR 3.

## The model

- A **managed account** is a config directory `~/.fletch/accounts/<provider>/<id>/`
  (`FLETCH_ACCOUNTS_ROOT` overrides the root in tests). No sandboxed CLI
  runs in it: it is the login's host-only storage, used by the host's own
  runs (the login PTY, the limits app-server). The directory listing
  **is** the registry; there is no accounts table. `id` is a slug
  (`[a-z0-9-]`, ≤32, not `default`) and doubles as the label.
  - A **codex** agent runs in its own `CODEX_HOME` overlay, never the
    account dir; the host copies the account's login into it without the
    refresh token (see "Codex: the host owns the login" below).
  - A **claude** account is a **token source only**. Its dir is host-only storage for
    the login (Settings signs in there with `CLAUDE_CONFIG_DIR`, and the
    Keychain item is named for it). Every claude agent, whatever its account,
    runs in the shared default config dir with `CLAUDE_CODE_OAUTH_TOKEN` set
    to the account's access token, which claude ranks above any `/login` in
    that dir. So all accounts' transcripts land in the default places.
- **The host owns every claude login** (`agent/host_login/claude.rs` on the engine in `agent/host_login`). A sandboxed
  claude can't refresh its own token: the refresh needs `mkdir <dir>.lock`,
  `<dir>/.oauth_refresh.lock` and a Keychain write, all denied under seatbelt,
  and a container has no Keychain. It also must not hold the ~30-day refresh
  token. So before every claude launch, on every engine, the app reads the
  account's login (Keychain item, else `.credentials.json`), refreshes it at
  `https://platform.claude.com/v1/oauth/token` when it expires within 30 min
  (`REFRESH_MARGIN_MS`), writes the rotated pair back where it came from
  (Keychain `-U` through `security -i`, or the file atomically at 0600,
  keeping every field it doesn't own), and hands the agent only the ~8h
  access token. Refresh tokens rotate, so refreshes run single-flight per
  account. `security -i` takes one line of about 4 KB, so a Keychain login
  too large to write back whole is not refreshed at all (the launch runs on
  the stored token while it lasts); a rotated pair the store refuses anyway
  is kept in memory and written on the next call. A refused refresh
  (400/401) marks the login revoked until the store changes, and Settings
  shows "sign in again"; the mark (the store's stamp, nothing secret) is
  kept under `<accounts root>/.state/claude-revoked/`, so it survives a
  restart. The token is resolved before the lifecycle lock
  and the spawn watchdog (`prefetch_login`), with a 5 s refresh timeout and
  a 20 s cap on a Keychain read that might prompt.
- **The sandbox holds no login but the access token.** Every agent's seatbelt
  profile denies reads of the host's claude logins on disk
  (`~/.claude/.credentials.json`, a relocated dir's, and every
  `~/.fletch/accounts/claude/` dir) and the Keychain's mach services
  (`KEYCHAIN_MACH_DENY`: `com.apple.SecurityServer`, which every login-keychain
  lookup goes through, plus `securityd`, `securityd.xpc`, `security.agent`).
  Measured on macOS 27: `security find-generic-password -s
  "Claude Code-credentials" -w` fails at once (exit 44, no prompt) under the
  profile. `com.apple.trustd` stays allowed, so TLS works for curl, node,
  python, git and Go (`gh`). codex is the exception: it reads its trusted
  roots from the Keychain and fails every request with `UnknownIssuer`, so a
  codex launch gets `CODEX_CA_CERTIFICATE=/etc/ssl/cert.pem` unless the user
  already sets `CODEX_CA_CERTIFICATE` or `SSL_CERT_FILE`. The trade-off is
  that a root a user added only to the Keychain (a corporate proxy's) is
  invisible to Keychain-verifying tools in the sandbox, and anything an agent
  kept in the Keychain (`gh auth`, git's `osxkeychain` helper) is unreachable
  there. A stdio MCP server still works under the profile (live test
  `live_seatbelt_turn_uses_a_stdio_mcp_server`). A remote MCP server that
  signs in with OAuth keeps its tokens in claude's Keychain item
  (`mcpOAuth`), which the sandboxed claude can no longer read: known limit,
  untested here (no such server is configured on this Mac).
- **A live claude can't take a new token** (it reads the env var once). Before
  a turn is delivered to an idle claude process whose token is inside the
  margin, the supervisor resolves a token first and, only when it lapses
  later than the live one, stops the process and resumes the session on it
  (`Supervisor::relaunch_with_resume` / `relaunch_locked`). Offline, the turn
  goes to the current process. A turn whose error result is a 401 gets one
  refresh, relaunch and resend (`supervisor/login_refresh.rs`). A second 401
  leaves the agent in `Error` with "sign in again"; the user's next send gets
  its own retry, which is how a new sign-in reaches the process. Per-turn
  providers start a process every turn and need none of this.
- **Sessions from before this change** live under the account's
  `projects/`. A seatbelt launch moves such a session (transcript and
  subagent dir) into the default projects dir before resuming it
  (`transcripts::adopt_account_session`), never overwriting one already
  there.
- The **default account** is the user's own CLI dir (`~/.claude`, `~/.codex`, or
  wherever their shell's env points). It has no directory under the root and no
  env override. It is also the **shared source**: `settings.json`, `CLAUDE.md`,
  `commands/`, `skills/`, `agents/`, `plugins/` (claude) and `config.toml`,
  `AGENTS.md`, `prompts/`, `skills/` (codex) are **symlinked** into each managed
  dir, never copied. Login, `.claude.json`, sessions stay per dir. (The claude
  links only matter to the Settings sign-in now, since agents don't run there.)
- The **active account** per provider is the settings key
  `provider_account_<provider>` (absent/blank/`default` = default). It is read
  **only at agent creation** and stamped on the record
  (`workspaces.provider_account`, migration 0049). Every later spawn uses the
  stamp. Switching the radio moves new agents only. The stamp itself moves only
  through `switch_agent_account` (the agent header's account picker), which
  the host refuses mid-turn; the next turn runs under the new account in the
  same workspace and conversation. Usage is credited by this stamp too: a
  session Fletch ran belongs to its workspace's account
  (`workspace::session_accounts`), and only transcripts matching no known
  session fall back to "whose directory holds it".
- **Switching a workspace's account** (`Supervisor::switch_account`, command
  and remote op `switch_agent_account` with `{ agentId, account }`) restamps
  `workspaces.provider_account` and, when the agent has a live handle,
  relaunches it on its own session (`relaunch_locked`), so the next turn runs
  on the new account's token in the same conversation. Refused mid-turn (the
  composer's busy notion: spawning or running; resting and errored sessions
  switch), for the current account (`None`, `""` and `default` are one), for
  an id with no directory, for a target whose probe reads signed out
  (`unknown` passes), and for an archived agent. Both providers switch: no
  CLI runs in an account dir (claude signs in by token, codex runs in its
  per-agent overlay), so a session resumes under any account. A codex switch
  takes effect at the next turn, whose launch credential is copied from the
  new stamp's login. Attribution follows the current stamp, so after a
  switch the session's whole history and its last limits reading count for
  the new account (for codex, until the next turn records a reading under it).
  Order: existence check, the provider's account lock
  (`accounts::AccountLocks` on `EngineCtx`, also held by removal across its
  check and delete and by sign-out across the logout, so neither can end an
  account between the switch's check and its restamp), the agent's delivery lock (a send's, so a message
  sent during the switch waits and runs under the new stamp), an input route
  (an archive can't tear the checkout down under the relaunch), then the
  checks, the probe and the target's token (`prefetch_login_as`) off the
  app-wide lifecycle lock; then, under it, the record is re-read and the
  checks and the busy test run again before the restamp. The kept token
  carries the stamp it was resolved for, so no launch under another stamp
  uses it. A failed relaunch puts the old stamp and the old rejected-token
  mark back, drops the target's token, and says why; a busy `take_idle`
  reads as "wait for the turn to finish". A session with no process just
  takes the stamp, and an `Error` left from the old account is cleared. A
  per-turn agent's live handle freezes the account into its `PerTurnSpec`, so
  it is rebuilt the same way (no process to stop between turns). Usage
  follows the current stamp, so a switch moves the workspace's whole past
  spend to the new account (accepted for v1). Emits `workspace:changed`. The
  UI's `switchAccount` gate needs `list_provider_accounts` on the wire as
  well, since the list it picks from is read with `invokeLocal` (this Mac's).
- Claude's macOS Keychain item for a managed dir is
  `Claude Code-credentials-<first 8 hex of sha256(dir path string)>`, no trailing
  slash, hashed exactly as the env var carries it. Verified against a live item
  on Alex's machine; unit-tested with fixed vectors.
- Only `claude` and `codex` have accounts (`accounts::ACCOUNT_PROVIDERS`). Other
  providers keep their single sign-in UI.

## File map

Engine (`crates/fletch-core/src/`):

- `agent/accounts.rs` — the module. `accounts_root`, `account_dir`,
  `validate_account_id`, `list_account_ids`, `list_account_dirs`,
  `shared_source_dir`, `ensure_account_dir` (mkdir + link repair + seeds
  `.claude.json` with `hasCompletedOnboarding`/theme for claude),
  `remove_account_dir` (also deletes the account's Keychain item),
  `account_env`, `ambient_credential_vars`, `account_for_new_agent`,
  `stamped_account_dir`, `existing_account_dir` (errors on a removed stamp),
  `list_accounts` (probes every account), `ProviderAccount`,
  `ACTIVE_SETTING_PREFIX`,
  `with_test_root` (test helper, crate-wide env lock).
- `agent/host_login/mod.rs` — the engine every host-owned login runs on:
  load, due check, single-flight refresh, write-back, a kept rotated login
  when the write fails (`Kept`, by store stamp), the refused-refresh mark.
  One rule after a refusal for every provider: if the stored refresh token
  changed while the request was out (another process rotated it), the
  rotated login is taken as freshly read, launched on unless it is due,
  else refreshed once more; a second refusal is the login's. Providers
  implement `LoginProvider` (store, parse, margin, refresh request,
  launch form, what a refusal is recorded against).
- `agent/host_login/claude.rs` — the claude adapter. `launch_token(account,
  rejected)` (what every claude launch signs in with; a managed account without
  a usable login fails the launch, the default degrades to no token),
  `access_token_for_launch`, `replace_rejected_token` (after a 401),
  `is_revoked` (free unless a refusal is on record; then the store's stamp
  only), `stored_access_token` (the "is this a login" bar, `expiresAt > 0`),
  `REFRESH_MARGIN_MS`, `AccessToken` (`Debug` prints the expiry only).
  `LoginStore` (`load` returns where it read from, `save` writes there,
  `fits` gates a refresh) and `TokenEndpoint` are the seams the tests mock.
  Ignored live tests: `live_forced_refresh_*`, `live_seatbelt_turn_*`,
  `live_legacy_account_session_*` (`FLETCH_LIVE_CLAUDE_ACCOUNT=<id>`).
- `agent/auth_probe.rs` — `probe_default`, `probe_dir` (per-account sign-in
  probe; Keychain presence or `.credentials.json` for claude, `auth.json` for
  codex; never consults shell keys). A claude login the host (`host_login::claude`) found
  revoked reads as signed out, "sign in again".
- `commands/accounts.rs` — `list_provider_accounts_impl`,
  `add_provider_account_impl`, `ensure_account_removable` (refuses the active
  account and any account a live agent is stamped with; reads the current
  stamp, so it follows a switch), `remove_provider_account_impl`,
  `set_active_provider_account_impl`,
  `sign_out_provider_account_impl` (runs the CLI's own logout — pinned in
  `agent/login.rs` `logout_command` — with the account's config-dir env; the
  account and its sessions stay), `switch_agent_account_impl` (desktop
  command and remote op share it).
- `supervisor/account_switch.rs` — `Supervisor::switch_account`: the
  refusals (`target_stamp`, `ensure_signed_in` over
  `accounts::probe_account`), restamp, relaunch, rollback. Ignored macOS tests
  in `account_switch/tests/launched.rs` run a scripted `claude` under
  sandbox-exec and compare token digests across the switch
  (`FLETCH_LIVE_SWITCH=<from>:<to>` for real accounts).
- `remote/dispatch.rs` — `switch_agent_account` op (`agents` scope).
- `workspace/agents.rs` — `live_agents_on_account` (count query over
  `workspaces` + `sessions`, where the provider lives); `session_accounts`
  (provider session id → stamped account, for the usage scan);
  `WorkspaceManager::update_agent_account` (the restamp).
- `supervisor/lifecycle.rs` — `active_account` stamping in `spawn_agent`;
  `record.account` threaded into `SpawnSpec`/`PerTurnSpec`;
  `spawn_agent_process` signs every claude launch in through `launch_login`
  into `SpawnSpec.oauth_token`; `start_process` adopts a legacy account-dir
  session; the managed event handler feeds `observe_login`.
- `supervisor/login_refresh.rs` — `prefetch_login` / `prefetch_login_as` /
  `launch_login` (token resolved before the lock and watchdog, kept with the
  stamp it is for, consumed by a launch under that stamp),
  `relaunch_with_resume` and `relaunch_locked` (for a caller already holding
  the lifecycle lock), `take_idle` + `restart_taken` (shared with
  `respawn_agent_preserving_session`: idle check and removal under one
  `agents` lock), `relaunch_if_login_due` (before a turn, from
  `send_user_message`), `observe_login` (401 → flag a session-preserving
  respawn on a replaced token and requeue the turn, once),
  `report_login_failure`, `forget_logins` (teardown). Scripted ignored tests
  in `login_refresh/tests/launched.rs`.
- `agent/spawn.rs` — `account_launch` → `AccountLaunch { dir, env, unset }`
  used by all four launch paths (claude PTY, claude managed, per-turn PTY,
  per-turn exec). `unset` strips the default account's credential vars; a
  claude account gets no `dir` and no env (it signs in by token).
  `Agent::login_expires_at_ms` is the launched token's expiry.
- `pty_session.rs`, `managed_session.rs`, `exec_session.rs` — `env_remove`
  field applied after every env layer.
- `sandbox/engine.rs` — `AgentLaunchCtx.account_dir` (codex only, naming the
  login; no engine grants or mounts it), `AgentLaunchCtx.codex_home` (codex's
  per-agent overlay) and `AgentLaunchCtx.oauth_token` (claude, every engine).
- `sandbox/seatbelt.rs` — puts `oauth_token` in the plan's env as
  `CLAUDE_CODE_OAUTH_TOKEN`; grants nothing for any account dir;
  `deny_host_claude_logins` hides the on-disk claude logins,
  `deny_host_codex_logins` the codex ones (the host's refresh temp files
  included), and `KEYCHAIN_MACH_DENY` the Keychain, from every agent (kernel
  tests `seatbelt_hides_the_hosts_claude_logins`,
  `seatbelt_hides_the_hosts_codex_logins`; HTTPS in
  `seatbelt_keeps_https_working_under_the_keychain_deny`); `codex_ca_env`
  gives codex a CA bundle (live test
  `live_codex_turn_runs_under_the_keychain_deny`). The codex overlay
  (`AgentLaunchCtx.codex_home`) is granted whole bar its `config.toml`. The
  relocated-claude-dir grants (islands + `.claude.json` literal +
  `settings.json` deny) stay: they still serve an app env that sets
  `CLAUDE_CONFIG_DIR`. The codex overlay is granted whole with `config.toml`
  denied; every profile denies reading `~/.codex/auth.json` and
  `<accounts root>/codex` (`deny_host_codex_logins`). Ignored kernel tests
  `seatbelt_enforces_account_dir_grants`, `seatbelt_hides_the_hosts_codex_logins`.
- `sandbox/container/{launch.rs, auth.rs, launch_auth.rs, config_dir.rs}` —
  `auth::resolve(oauth_token)`: the host token (`AuthSource::HostLogin`), then (default account only,
  since a managed launch always has a token) the stored setup-token and the
  shell's auth vars. A claude account dir is never mounted; the per-agent
  `.fletch-claude-projects` mount stays; the `.credentials.json` rw overlay
  is dropped whenever a host token is injected. `status()` is presence-only.
  `claude_keychain_service`. Codex: the overlay mounted read-write and
  forwarded, the shared config it links to mounted read-only, never
  `~/.codex` or the account dir.
- `keychain.rs` — `item_present`, `item_stamp` (`acct` + `mdat`, no secret),
  `read_password` (launch or explicit action only, 20 s cap),
  `write_password` (`security -i`, hex `-X`, so the secret never hits argv;
  refuses a password over the ~4 KB line `password_fits` allows; fixed
  error text, never `security`'s stderr), `delete_item`. The in-process
  Keychain API was tried and rejected: a read of a `security`-created item
  from the test binary took ~13 s on first access, the signature of an
  access prompt.
- `transcripts.rs` — `claude_projects_dirs` unions the claude account dirs
  (old claude history still lives there);
  `claude_projects_dir(cwd, container)` resolves to the default for every
  claude agent; `adopt_account_session` moves a legacy session out of an
  account dir. Codex: `codex_overlay_sessions_dirs` (every agent's own, by
  agent id), `legacy_codex_sessions_dirs` (default + account homes, threads
  from before overlays), `find_codex_rollouts(id, agent_id)` (the agent's overlay,
  then legacy).
- `agent/host_login/codex.rs` — the codex adapter on the engine: the
  `auth.json` store (stamped by mtime and size), JWT expiry and the
  min(24h, half the lifetime) margin, the auth.openai.com refresh and its
  error codes, the launch copy with the refresh token blanked
  (`launch_file`), the refused-token mark (`is_revoked`, read by
  `auth_probe`, kept in `.state/codex-signed-out` as before).
- `agent/codex_home.rs` — the per-agent overlay: `overlay_for_agent`,
  `open_overlay`, `prepare_overlay`, `write_launch_credential` (the
  launch file written through a no-follow handle, removed when nothing is
  stored or the login was refused). `agent/credential_file.rs` — the 0600
  fsynced atomic write, the file stamp and `PrivateDir`.
- `supervisor/materialize.rs` — fork/rewind writer: the default dir for
  claude, the agent's overlay for codex.
- `usage_scan/` — scans all roots (incl. account dirs and agent overlays);
  each bucket and session carries an `account`: the workspace stamp for a
  session Fletch ran (`Attribution::sessions`); a codex agent's overlay by
  the agent's own stamp (`Attribution::roots`), so its sub-agent threads go
  with it; anything else by the dir that holds it. Codex rollouts'
  `rate_limits` surface as `UsageScan::rollout_limits`, credited the same way.
- `agent/limits/` (PR 3) — `ProviderLimits`/`AccountLimits`, normalisers per
  source, the `provider_limits_<provider>_<account>` row (`record_limits`,
  `record_refresh`, refresh floor and 429 back-off), `app_server` (codex
  on-demand read), `oauth_usage` (claude manual refresh, on
  `host_login::claude`'s refreshed token; a 401 gets one forced refresh).
- `commands/limits.rs` (PR 3) — `get_provider_limits_impl`,
  `refresh_provider_limits_impl`, `scan_usage_transcripts_impl` (the scan plus
  storing rollout readings; desktop and remote both call it).
- `commands/settings.rs` — `is_host_setting_key` admits the
  `provider_account_*` family.

Desktop (`src-tauri/src/commands/`): `accounts.rs` (thin wrappers; kills the
account's login PTY before removal, after the removability check;
`switch_agent_account`),
`provider_login.rs` (`open_provider_login(id, account, cols, rows)`,
`session_key(provider, account)` = `provider` or `provider:account`).

Frontend (`src/`):

- `api/types/providers.ts` — `ProviderAccount`, `DEFAULT_ACCOUNT_ID`.
- `api/domains/providers.ts` — `listProviderAccounts`, `addProviderAccount`,
  `removeProviderAccount`, `setActiveProviderAccount`,
  `openProviderLogin(id, cols, rows, account?)`. All `invokeLocal` (this Mac).
- `store/providers.ts` — `providerAccounts: Record<provider, ProviderAccount[]>`,
  `refreshProviderAccounts` (rides along with `refreshProviderAuth`, which the
  4 s providers poll drives), add/remove/setActive actions. Tests in
  `store/providerAuth.test.ts`.
- `data/providerAccounts.ts` — `accountSlug`, `accountLabel`.
  `data/providerDetail.ts` — `accounts: true` + `supportsAccounts()`.
- `components/SettingsScreen/ProvidersPane/AccountsSection/{index,AccountRow,AddAccountForm}.tsx`
  — the list. `ProviderRow` renders it for account providers, else
  `SignInSection`. CSS block `.set-prov-acct*` in `SettingsScreen.css`.
- `components/SettingsScreen/ProviderLogin/loginSessions.ts` — `loginKey()`;
  `ProviderLoginTerminal` takes `accountId`.
- `api/types/agent.ts` — `AgentRecord.account`.
- `api/domains/agents.ts` — `switchAgentAccount(agentId, account)` (host
  `invoke`); store action `switchAgentAccount` in `store/workspace.ts`, queued
  on `configOps` with effort/model so a send right after runs under the new
  account. It writes only `account` back (status stays with `agent:*`
  events) and holds `switchingAccount[id]` while in flight. Gate
  `switchAccount` needs both `list_provider_accounts` and
  `switch_agent_account` on a remote host.
- `components/Workspace/AccountPicker/` — header picker (`index.tsx` on
  `ui/MenuButton`'s `trigger`, `AccountMenu.tsx`, `choices.ts`,
  `useCanSwitchAccount.ts`) and `SwitchAccountHint` above the composer when
  the last turn failed on a limit or sign-in (`accountError.ts`). The header
  trigger hides while at most one account is not signed out; the hint then
  links to Settings. CSS `.acct-pick*` / `.acct-hint*` in `Workspace.css`.

## Codex: the host owns the login (`feat/codex-host-login`)

Principle (owner's, not to reopen): a sandboxed agent never holds a long-lived
credential and never manages its own login. The host signs in, stores and
refreshes; a launch gets the shortest-lived credential that works. Claude's
half is `feat/claude-host-login`.

What codex's login is (codex-cli 0.154.0, `codex-rs/login/src/auth/manager.rs`):

- `auth.json`: `auth_mode` (`"chatgpt"`), `OPENAI_API_KEY` (null for a
  subscription login), `tokens.{id_token, access_token, refresh_token,
  account_id}`, `last_refresh` (RFC 3339, stamped on every refresh).
- `access_token` is the bearer sent to the API: a JWT with a **ten-day**
  lifetime (`exp - iat = 864000`). `id_token` lives one hour and is only read
  for its claims (an expired one is fine). The refresh token is opaque
  (`rt.…`), no readable expiry, and **rotates**: a refresh may return a new one,
  and reusing a spent one is refused (`refresh_token_reused`).
- codex refreshes on its own only when the access token is within five minutes
  of `exp` (or, with no readable `exp`, when `last_refresh` is over eight days
  old): `POST https://auth.openai.com/oauth/token`, JSON body
  `{grant_type: "refresh_token", client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
  refresh_token}`; the answer is `{id_token?, access_token?, refresh_token?}`.
  A bad refresh token gets `401 {"error": {"code": "invalid_refresh_token"}}`.
- `codex exec` and `codex exec resume <id>` run normally from an `auth.json`
  whose `refresh_token` is `""`. With the field **absent** the whole file is
  rejected ("Missing bearer or basic authentication"). With an expired access
  token and an empty refresh token the turn fails:
  `Failed to refresh token: 400 Bad Request: Invalid 'refresh_token': empty string`.
- No other hook takes a ChatGPT access token for `codex exec`.
  `CODEX_ACCESS_TOKEN` is for personal access tokens (`at-…`) and Agent
  Identity JWTs, not the ChatGPT OAuth token; `OPENAI_API_KEY`/`CODEX_API_KEY`
  are API billing. The app-server has an externally-managed mode
  (`account/login/start` with `chatgptAuthTokens`, refreshed through the
  `account/chatgptAuthTokens/refresh` server request), which would let the
  host hand over a token per request; it needs Fletch to drive turns through
  the app-server instead of `exec`, so it is a later option, not this PR.
- Sessions: `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`, with
  `state_5.sqlite`, `thread_history_1.sqlite`, `logs_2.sqlite` and friends
  beside them. `codex exec resume <id>` finds a rollout that is in no sqlite
  index (a fresh `CODEX_HOME` holding only the copied rollout resumed it).

What Fletch does:

- Every codex launch (per-turn exec, native TUI; seatbelt and containers) runs
  with `CODEX_HOME=<agent dir>/.fletch-codex-home` (`overlay_for_agent`, the
  one path the launch, the fork/rewind writer and the locator share), an
  overlay the host assembles before each launch: `sessions/` (the agent's own), the shared
  `config.toml`/`AGENTS.md`/`prompts`/`skills` linked in from the user's codex
  home (`accounts::shared_items`; anything the agent put in
  their place is replaced), and `auth.json`.
- `auth.json` in the overlay is the account's login with `refresh_token: ""`,
  every other field kept. It is rewritten before the launch **and before every
  turn**. When the access token has less than a day left (or half its
  lifetime, if shorter), the host refreshes first and writes the result back
  to the account's own `auth.json` atomically (0600, fsynced, through a
  `.fletch-auth-*` temp file the sandbox can't read either, unknown fields
  kept). One refresh per login at a time; the file is re-read under the lock
  so a refresh the user's own codex just made is used, not repeated, and a
  refusal of a refresh token that another process rotated meanwhile is
  retried once with the rotated one.
- Every host write into the overlay (the credential, the shared links,
  adopted and forked threads) goes through a handle opened without following
  links (`credential_file::PrivateDir`): an agent that swaps the overlay, or
  anything in it, for a symlink between turns gets a real directory back,
  and the host never writes through the link.
- **Fletch rotates the user's personal codex login.** For the default
  account the login is `~/.codex/auth.json`, and Fletch refreshes it up to a
  day before expiry. A codex the user is already running keeps working:
  before any refresh, proactive or after a 401, codex re-reads `auth.json`
  and adopts a token another process wrote there instead of spending its
  own (`AuthManager::refresh_token`'s guarded reload in
  `codex-rs/login/src/auth/manager.rs`; the 0.154.0 binary carries its
  "Skipping token refresh because auth changed after guarded reload" log).
- A refused refresh (401, or `refresh_token_expired`/`_reused`/`_invalidated`,
  `invalid_grant`, `invalid_refresh_token`) fails the turn with "sign in again"
  and records a signed-out mark (`<accounts root>/.state/codex-signed-out/`,
  holding only a hash of the refused refresh token); Settings then shows the
  account signed out until a new sign-in replaces the token. A transient
  failure launches with the current token while it is valid and fails the
  turn once it has expired.
- The account's directory (or `~/.codex`) is host-only storage. Seatbelt no
  longer grants `~/.codex`; it grants the overlay (with its `config.toml`
  denied) and denies **reading** `~/.codex/auth.json` and `<accounts
  root>/codex/` in every agent's profile. Containers mount the overlay and the
  shared config read-only; never the home or the account dir.
- Transcripts move with the agent, not the account: rollouts are found in the
  agent's overlay first. A thread an agent ran before this change is copied
  (with its sub-agent threads) into the overlay at its launches, each file
  whole under its final name and every missing one retried until the whole
  thread is confirmed (a marker in the overlay's `.fletch-adopted/` then
  skips the legacy walk), so resume keeps working; the original stays put, and the usage scan counts a rollout name
  once, so the copy isn't spend twice. The usage scan credits an overlay to
  the agent's stamp (`workspace::agent_accounts`). The fork/rewind writer
  writes into the target agent's overlay.
- Unchanged: the limits app-server still runs on the host with
  `CODEX_HOME=<account dir>`; managed launches still strip `OPENAI_API_KEY`.

## PR 3: per-account usage and limits

### Goal

In Settings › Providers each account row shows the two windows the vendor's
own usage page shows — five-hour and weekly — as **percent used + reset time**
(neither vendor exposes token counts against the limit; the pages don't
either), plus a freshness stamp. The Usage screen gains per-account spend rows.
Accounts included: default and managed, for claude and codex.

### Decisions already made (do not reopen)

- Claude limits with no agent running: **manual refresh only** via the
  undocumented OAuth usage endpoint, with a cache floor and 429 back-off. No
  background polling. Alex approved reading the account's access token for a
  user-initiated refresh; it must never be logged or cross IPC.
- Codex limits: on demand via the app-server, no model quota spent.
- No auto-switching.

### Data sources and shapes

**Codex, on demand (primary).** Spawn `codex app-server` (binary via
`agent::resolve_agent_bin("codex", …)`; honours bin overrides) with
`CODEX_HOME=<account dir>` for a managed account (nothing for default) and the
default account's `OPENAI_API_KEY` **removed** for managed accounts
(`accounts::ambient_credential_vars`). JSON-RPC 2.0 over stdio, newline
delimited, no Content-Length framing:

```
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"fletch","version":"…"}}}
← {"jsonrpc":"2.0","id":1,"result":{…}}
→ {"jsonrpc":"2.0","id":2,"method":"account/rateLimits/read","params":null}
← {"jsonrpc":"2.0","id":2,"result":{"rateLimits":{
     "primary":  {"usedPercent":42,"windowDurationMins":300,  "resetsAt":1788265323},
     "secondary":{"usedPercent":61,"windowDurationMins":10080,"resetsAt":1788765541},
     "planType":"…", … }}}
```

`resetsAt` is epoch seconds. A 401-class failure surfaces as an untyped
`-32603` error: treat as "signed out". Use a ~10 s timeout (community tools saw
4 s drop intermittently). Kill the child after the read. Refresh on Settings
open and on a button; cache ≥60 s.

**Codex, passive (free).** Rollout files (`<CODEX_HOME>/sessions/YYYY/MM/DD/
rollout-*.jsonl`, already scanned by `usage_scan`) carry, on `event_msg`
`token_count` events, a `rate_limits` object:
`{primary:{used_percent, window_minutes, resets_at}, secondary:{…}}`. The
parser in `usage_scan/parse` and `src/adapters/codex/usage.ts` reads
`token_count` already and drops this field — the last one seen per root is a
free "last known" value. `codex exec --json` (the live stream) does **not**
emit limits.

**Claude, passive (free, exact).** Managed stream-json sessions emit:

```
{"type":"rate_limit_event","rate_limit_info":{
   "status":"allowed"|"rejected","resetsAt":<epoch s>,"rateLimitType":"five_hour",
   "unifiedWindows":{"five_hour":{"utilization":0.06,"resetsAt":…},
                     "seven_day":{"utilization":0.02,"resetsAt":…}}}}
```

`utilization` is a 0–1 fraction. The supervisor sees every managed event
(`supervisor/lifecycle.rs` `spawn_managed_agent` → `on_event`; `activity.rs`
classifies turn ends). Capture it there, where the agent record (hence the
account) is known, and record it per `(provider, account)`. The claude adapter
in `src/adapters/claude` currently ignores the event; it may keep ignoring it.

**Claude, status line hook (covers the native PTY view).** Since Claude Code
2.1.80 the JSON piped to a `statusLine` command includes
`rate_limits.five_hour.{used_percentage, resets_at}` and `.seven_day` (percent
0–100, epoch seconds), only for claude.ai Pro/Max logins. Fletch can inject a
status-line command at spawn through the `--settings <json>` flag that
`attribution::claude_settings_args` already uses — **merge into one JSON
object; `--settings` is passed once**. The command should write the JSON to a
file under the agent's RPC dir (`FLETCH_RPC_DIR`, writable in every engine) that
the supervisor tails. Design call to make and document: injecting a statusLine
overrides a user's own statusLine for Fletch-run sessions. Prefer injecting only
when the shared `settings.json` has none, or chain to theirs.

**Claude, on demand (manual refresh).** `GET https://api.anthropic.com/api/oauth/usage`
with `Authorization: Bearer <accessToken>` and `anthropic-beta: oauth-2025-04-20`.
Response: `five_hour {utilization, resets_at}`, `seven_day {…}`, optional
`seven_day_opus`, `seven_day_sonnet`, `extra_usage`. **`utilization` here is a
percent 0–100** (unlike the stream's 0–1) and `resets_at` is ISO 8601. The
token is the `claudeAiOauth.accessToken` of the account's credential JSON: the
Keychain item named by `container::auth::claude_keychain_service(Some(dir))`
(default: `None`) read with `keychain_token(service)`, else
`<dir>/.credentials.json`. Today `keychain_token` is launch-path-only and
documented as such — PR 3 adds a second, explicitly user-initiated caller;
update that doc. Access tokens expire in ~60 min and Claude refreshes them only
when it runs: on 401 report "stale, run an agent under this account to
refresh", do **not** implement the OAuth refresh flow. (Superseded by host
login 1: the host now refreshes, tokens last ~8h, and `oauth_usage` reads
through `host_login::claude`; see "The model".) On 429 back off
(exponential, persisted) and show the last known value. Never poll.

**Spend per account.** Add an `account` dimension to `usage_scan`: a record's
account is the managed id whose dir contains the transcript file, else
`default`. `UsageBucket` gains `account`; `UsageSessionSpan` too. Frontend
`src/data/usage/aggregate.ts` folds a per-account row set;
`UsageScreen/UsageBreakdown` shows it. Keep the wire shape additive.

### Storage

Last-known limits per `(provider, account)` as one JSON settings row,
e.g. key `provider_limits_<provider>_<account>` (admit the family in
`commands::settings::is_host_setting_key` like `provider_account_*`), holding
`{five_hour:{percent, resets_at}, seven_day:{…}, as_of, source}` with
`source ∈ {stream, statusline, app_server, oauth_usage, rollout}`. Normalise
every source to percent 0–100 and epoch seconds at the boundary. Emit
`settings:changed` (`commands::settings::announce`) so the pane updates without
polling.

### UI

- `AccountRow`: two compact meters (five-hour, weekly) with percent and
  "resets 14:30 · in 2h 13m", a muted "as of N min ago · <source>" line, a
  per-provider Refresh in the `AccountsSection` head. Claude rows show a
  "sign in to see limits" hint when no data and signed out. Reuse `Badge`,
  `Button`, `SetSeg`; CSS beside `.set-prov-acct*`.
- Usage screen: provider rows split by account when a provider has more than
  one; otherwise unchanged.
- No new nav entries.

### Verification in this sandbox (read before running anything)

- Repo root has no `Cargo.toml`. From `src-tauri/`: `cargo check`,
  `cargo clippy --all-targets -- -D warnings -A clippy::significant_drop_tightening`,
  and **also** `cargo clippy -p fletch-core --lib -- -D warnings -A clippy::significant_drop_tightening`
  (the desktop run lints only its own crate; CI lints fletch-core separately —
  `type_complexity` on a tuple return already bit once). `cargo check -p fletch-core --tests`
  compiles engine tests (ignore pre-existing `start_paused` tokio errors).
  `cargo fmt --manifest-path crates/fletch-core/Cargo.toml --check` and the
  same for `src-tauri`.
- `cargo test --manifest-path crates/fletch-core/Cargo.toml` **cannot run
  here** (crate downloads into `~/.cargo/registry` are blocked; `--offline`
  fails resolution). Don't relocate `CARGO_HOME`. List the engine tests that
  need CI in the report. `cd src-tauri && cargo test` does run.
- Frontend: `bun install --frozen-lockfile` once, then `bun run check`,
  `bunx biome check --diagnostic-level=error` (many pre-existing warnings are
  fine), `bunx vitest run <files>`, `bun run build`. `bunx biome format --write`
  on touched files.
- `sandbox-exec` cannot be nested here; the ignored seatbelt test is Alex's.
- Network from this sandbox is not guaranteed; don't make PR 3's tests hit the
  vendors. Mock the app-server and the usage endpoint.

### Conventions that reviewers held us to

- Comments explain **why**; tests have descriptive snake_case names; one
  behaviour per test. Mirror constants across Rust/TS with a doc pointer each
  way (e.g. `ACCOUNT_PROVIDERS` ↔ `supportsAccounts`).
- Never log or return a credential value; `Debug` impls print var names only.
  Keychain reads with `-w` only on an explicit user action or launch, never on
  a polling path (`item_present` is the presence check).
- A managed account must never fall back to the default account's credentials:
  strip `ambient_credential_vars` on host launches, filter in the container
  chain, and the app-server probe must do the same.
- Don't hold a lock across an await (`await_holding_lock` is denied).
- `invokeLocal` for anything about this Mac (binaries, Keychain, files under
  `~/.fletch`); `invoke` for host-owned settings.
- Commit messages: conventional, end with
  `Co-Authored-By: Claude <noreply@anthropic.com>`. Pushes and PRs go through
  the Fletch RPC ops (`git_push`, `open_pr` with `base` set to the branch the
  work is stacked on).

### Known limits carried forward (mention in the PR, don't fix in PR 3)

- The launch credential is the ten-day access token, not something shorter:
  codex offers nothing shorter for a ChatGPT login under `codex exec`. The
  sandbox holds that token for as long as it lives; it never holds the
  refresh token.
- A native codex TUI gets its credential once, at launch; one left open past
  the access token's expiry fails its next request and has to be reopened.
  Per-turn agents get a fresh file every turn.
- A refresh can block a turn's start for up to 20 s (it runs on its own
  thread, about once every nine days per login).
- A rotated codex login whose save to `auth.json` failed lives only in the
  app's memory (`credential_file::Kept`), used by launches and saved by the
  next one that can. If the app quits before that retry, the kept login is
  lost, and the refresh token on disk is already spent: the account needs a
  new sign-in. A new sign-in made meanwhile wins over the kept login.
- If the user's own codex and Fletch refresh one login at the same instant,
  one of them spends a refresh token the other already rotated and codex's
  reuse detection may sign the login out. Both sides re-read the file before
  refreshing, and Fletch retries a refusal once with a token rotated
  meanwhile, which narrows this to truly simultaneous refreshes; it cannot
  close it across processes.
- Codex usage follows the current stamp: after a switch, the session's
  whole history and its last limits reading count for the new account until
  the next turn records a reading under it.
- `cli_auth_credentials_store = "keyring"` logins (no `auth.json`) are not
  read: such an agent runs signed out.
- Each agent's overlay grows its own codex state (`logs_2.sqlite` etc.). In
  containers the shared `skills` is read-only, so codex logs that it could
  not install its bundled system skills; the ones already installed on the
  host are read through the link. Seatbelt denies the same write, since
  `~/.codex` is no longer writable.
- Codex's `hooks.json` is not among the shared items, so user hooks don't run
  in Fletch codex sessions.
- One-shot CLI runs (handoff summariser) use the default account.
- Claude's `.claude.json` is read-only in containers (same as a non-default
  `CLAUDE_CONFIG_DIR` today); onboarding is pre-seeded to compensate.
- `ensure_account_dir` never overwrites a shared-config link the CLI replaced
  with a real file; Settings does not yet surface such a fork.
- A Keychain claude login over about 2 KB (the `security -i` line limit,
  hex-encoded) is never host-refreshed, so it needs a fresh sign-in each time
  its access token expires.
- A rotated pair the store refused (the engine's `Kept`) lives only in
  memory: if the app quits before a retry stores it, the login is lost and
  needs a new sign-in. Follow-up: persist it through `crate::secrets`.

## Sources consulted

- Claude Code docs, Authentication (multiple accounts via `CLAUDE_CONFIG_DIR`,
  per-dir Keychain item): https://code.claude.com/docs/en/authentication
- Claude Code status line docs (`rate_limits` fields):
  https://code.claude.com/docs/en/statusline
- `rate_limit_event` shape and SDK discussion: anthropics/claude-code#50518,
  #45133; OAuth usage endpoint 429 behaviour: #31021, #31637.
- Codex app-server `account/rateLimits/read` examples: Adarsh350/codex-usage,
  Prontsevich/codex-limits-mcp, openai/codex#48591 (401 → -32603);
  rollout `rate_limits` field: tech.hatada.jp "reading Codex rate limits
  without burning them".
