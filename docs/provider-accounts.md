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
  (`FLETCH_ACCOUNTS_ROOT` overrides the root in tests). The CLI is pointed at it
  with `CLAUDE_CONFIG_DIR` (claude) or `CODEX_HOME` (codex). The directory
  listing **is** the registry; there is no accounts table. `id` is a slug
  (`[a-z0-9-]`, ≤32, not `default`) and doubles as the label.
- The **default account** is the user's own CLI dir (`~/.claude`, `~/.codex`, or
  wherever their shell's env points). It has no directory under the root and no
  env override. It is also the **shared source**: `settings.json`, `CLAUDE.md`,
  `commands/`, `skills/`, `agents/`, `plugins/` (claude) and `config.toml`,
  `AGENTS.md`, `prompts/`, `skills/` (codex) are **symlinked** into each managed
  dir, never copied. Login, `.claude.json`, sessions stay per dir.
- The **active account** per provider is the settings key
  `provider_account_<provider>` (absent/blank/`default` = default). It is read
  **only at agent creation** and stamped on the record
  (`workspaces.provider_account`, migration 0049). Every later spawn uses the
  stamp. Switching the radio moves new agents only. The stamp itself moves only
  through `switch_agent_account` (the agent header's account picker), which
  the host refuses mid-turn; the next turn runs under the new account in the
  same workspace and conversation.
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
  `ACTIVE_SETTING_PREFIX`, `with_test_root` (test helper, crate-wide env lock).
- `agent/auth_probe.rs` — `probe_default`, `probe_dir` (per-account sign-in
  probe; Keychain presence or `.credentials.json` for claude, `auth.json` for
  codex; never consults shell keys).
- `commands/accounts.rs` — `list_provider_accounts_impl`,
  `add_provider_account_impl`, `ensure_account_removable` (refuses the active
  account and any account a live agent is stamped with),
  `remove_provider_account_impl`, `set_active_provider_account_impl`.
- `workspace/agents.rs` — `live_agents_on_account` (count query over
  `workspaces` + `sessions`, where the provider lives).
- `supervisor/lifecycle.rs` — `active_account` stamping in `spawn_agent`;
  `record.account` threaded into `SpawnSpec`/`PerTurnSpec`.
- `agent/spawn.rs` — `account_launch` → `AccountLaunch { dir, env, unset }`
  used by all four launch paths (claude PTY, claude managed, per-turn PTY,
  per-turn exec). `unset` strips the default account's credential vars.
- `pty_session.rs`, `managed_session.rs`, `exec_session.rs` — `env_remove`
  field applied after every env layer.
- `sandbox/engine.rs` — `AgentLaunchCtx.account_dir`.
- `sandbox/seatbelt.rs` — relocated claude dir: island grants + `.claude.json`
  literal + explicit deny on `settings.json`; codex account home granted whole
  with `config.toml` deny. Ignored kernel test documents the temp-tree grant.
- `sandbox/container/{launch.rs, auth.rs, launch_auth.rs, config_dir.rs}` —
  account dir mounted and forwarded; `auth::resolve(account_dir)` reads the
  account's suffixed Keychain item / `.credentials.json` only;
  `claude_keychain_service`, `keychain_token(service)` (a launch, or the
  limits Refresh through `oauth_access_token`; never a polling path).
- `keychain.rs` — `item_present`, `delete_item` (macOS; presence-only reads).
- `transcripts.rs` — `claude_projects_dirs` and `codex_sessions_dirs` union the
  account dirs; `claude_projects_dir(cwd, container, account_dir)`.
- `supervisor/materialize.rs` — fork/rewind writer targets the stamped dir via
  `existing_account_dir`.
- `usage_scan/` — scans all roots (incl. account dirs); each bucket and
  session carries the `account` whose dir holds the transcript (PR 3), and
  codex rollouts' `rate_limits` surface as `UsageScan::rollout_limits`.
- `agent/limits/` (PR 3) — `ProviderLimits`/`AccountLimits`, normalisers per
  source, the `provider_limits_<provider>_<account>` row (`record_limits`,
  `record_refresh`, refresh floor and 429 back-off), `app_server` (codex
  on-demand read), `oauth_usage` (claude manual refresh).
- `commands/limits.rs` (PR 3) — `get_provider_limits_impl`,
  `refresh_provider_limits_impl`, `scan_usage_transcripts_impl` (the scan plus
  storing rollout readings; desktop and remote both call it).
- `commands/settings.rs` — `is_host_setting_key` admits the
  `provider_account_*` family.

Desktop (`src-tauri/src/commands/`): `accounts.rs` (thin wrappers; kills the
account's login PTY before removal, after the removability check),
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
  trigger hides with one account or fewer; the hint then links to Settings. CSS
  `.acct-pick*` / `.acct-hint*` in `Workspace.css`.

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
refresh", do **not** implement the OAuth refresh flow. On 429 back off
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

- In containers a codex account's links to shared config point at `~/.codex`,
  which isn't mounted, so in-container codex runs without that shared config.
- One-shot CLI runs (handoff summariser) use the default account.
- Claude's `.claude.json` is read-only in containers (same as a non-default
  `CLAUDE_CONFIG_DIR` today); onboarding is pre-seeded to compensate.
- `ensure_account_dir` never overwrites a shared-config link the CLI replaced
  with a real file; Settings does not yet surface such a fork.

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
