# Fletch multi-host: verified and refined plan

2026-09-18. Revision of "Fletch Multi-Host Architecture: Verified Implementation
Plan" (same date). Every claim below was re-checked against `fwdai/fletch` at
`458a88cf` (v0.7.31) and against T3Code at `d4d5d12e` (2026-09-17,
`github.com/pingdotgg/t3code`). File paths are relative to the repo root unless
they start with `t3code/`.

Goal, unchanged: any machine runs a Fletch host that executes agents; any Fletch
client (desktop or phone) controls one or more hosts; the desktop works as host,
client, or both.

## 1. Audit of the original doc

The original is accurate in its architecture reading. The host stack really is
`RemoteState` (no `AppHandle`) + `SupervisorDispatch` (41 ops) + 15 forwarded
events + Noise XX over WebSocket + a dumb Cloudflare relay, and the engine really
uses the `AppHandle` as an event emitter plus a service locator. Its plan is
sound in shape. What follows is only what was wrong, understated, or missing.

### 1.1 Wrong or imprecise claims

| Original claim | Actual | Where |
| --- | --- | --- |
| 53 of 255 non-test `.rs` files mention `AppHandle` | 53 of **256** (excluding `*tests*.rs` and `*/tests/*`); 285 if only `*tests*.rs` is excluded | `src-tauri/src` |
| `workflow/` 48 mentions, `remote/` 9 | **52** and **11** | per-file counts in §1.4 |
| `try_state` called 2 times, both `roadmap::Db` | **7** calls: 3 fetch `Arc<Supervisor>`, 1 `Arc<WorkflowService>`, 2 `roadmap::Db`, 1 `Arc<Supervisor>` in `remote/push.rs:301`. The doc lists all seven sites elsewhere, so only the summary line is wrong | `supervisor/messaging.rs:522,548`, `roadmap/review.rs:272`, `workflow/comms/mod.rs:425`, `supervisor/lifecycle.rs:851,1087`, `remote/push.rs:301` |
| Only `emit`, `try_state`, `listen_any`, `get_webview_window` are used | True for `AppHandle` methods, but the engine also uses **`tauri::async_runtime::spawn` (22 sites)**, **`tauri::State` (43, in `roadmap/` and `workflow/` command fns)**, `tauri::Manager` and `tauri::Listener` traits. Phase 1 must replace `async_runtime::spawn` with `tokio::spawn`; the doc never mentions it | `supervisor/` 14, `workflow/` 3, `roadmap/` 5 |
| `workflow/scheduler` already treats the handle as optional | `RunCtx`/`ChildCtx` do, but `WorkflowService.app: AppHandle` and `scheduler/drive.rs:42` are non-optional. The module is easier than the others, not already done | `workflow/scheduler/mod.rs:66,111,133` |
| Supervisor methods taking `AppHandle`, "e.g." five names | **19 public methods**, about 10 crate-private ones, ~20 free fns and ~22 `emit_*` helpers. This is the bulk of Phase 1, not a footnote | `supervisor/lifecycle.rs`, `messaging.rs`, `shell.rs`, `fork.rs`, `run.rs`, `session_sync.rs`, `disposition.rs`, `events.rs` |
| 43 distinct backend event names | **~50** (21 supervisor, 13 `roadmap:*`, 3 `wf:*`, 4 `dictation:*`, 9 misc) | `supervisor/events.rs`, `roadmap/client_events.rs`, `workflow/journal.rs`, `lib.rs`, `oauth.rs`, `commands/*` |
| 3 files bypass `src/api/events.ts` with raw `listen` | **5**: add `Composer/useFileDrop.ts` and `SettingsScreen/Dictation/useDictationModel.ts` | `src/components/...` |
| `src/api/domains/` has 14 domains | **18**: also `commands`, `dictation`, `remote`, `workspace` | `src/api/domains/` |
| Relay mux framing is `0x01` OPEN, `0x02` DATA, `0x03` CLOSE | Also **`0x04` TEXT** and **`0x05` NOTIFY** (APNs push request, connId 0) | `relay/src/frames.ts:1-5` |
| Setup closure spans lines ~1450–1800 | `1449–1870`. The doc's bootstrap list omits: approval and publish prefs seeding, `remote::push::set_turn_complete`, docker/podman launch settings and version-refresh guards, container-auth mirror, `git_dist::init` + startup, whisper init, GitHub/Linear token seeding, `docker::set_build_sink`, telemetry, `app.manage(db)`, `ClaudeSetupState`/`ProviderLoginSessions`, tray, orphan sweeps, and an **existing SIGINT/SIGTERM handler** that calls `supervisor.shutdown()` | `src-tauri/src/lib.rs:1488-1510, 1530-1629, 1634-1680, 1819-1865` |
| `seatbelt.rs` "already gated at lines 908 and 1589" | Those two `cfg(target_os = "macos")` gate **ignored acceptance tests only**. `sandbox/` has no platform gates; it compiles on Linux and fails at spawn because `/usr/bin/sandbox-exec` is hardcoded | `sandbox/seatbelt.rs:100,901-908,1589` |
| CI runs fmt, clippy, test on Ubuntu | Yes, plus a Bun job (lint, typecheck, build, test); clippy runs `-D warnings -A clippy::significant_drop_tightening`; everything uses `--manifest-path src-tauri/Cargo.toml` because there is no workspace | `.github/workflows/ci.yml` |
| Dispatcher arms "call the same function the matching Tauri command calls" | They call shared `*_impl` helpers (e.g. `commands::commit_agent_impl`), not the `#[tauri::command]` fn. Same code path, different symbol. This is the pattern Phase 4 generalises | `remote/dispatch.rs:283`, `commands/git_ops.rs:43` |
| G4: mobile dialer holds one connection | Also true one level up: the **mobile TS store and persistence are single-host** (`hostKey: string \| null`, "the last host"). Multi-host on the phone is a store rewrite, not only a Rust map | `mobile/src/store/index.ts:67-78`, `mobile/src/store/persist.ts:1-27` |

### 1.2 Open questions in the original, now answered

- **PTY scrollback on the host: none for agent and shell PTYs.** `pty_session.rs`
  is a thin `portable_pty` wrapper; `agent:output` is emitted per PTY chunk
  (`supervisor/lifecycle.rs:1575` → `events.rs:35`) and forgotten. The only
  replay buffer is in the **frontend**: `src/pty/channel.ts` keeps a 256 KiB
  ring per agent, replayed on first mount and documented as lossy. Run sessions
  are the exception: `run_session.rs` keeps a 5 MiB backend log exposed by the
  `run_state` command (not on the remote allowlist). Phase 5 therefore needs a
  host-side retained buffer. See §3.5 for why a byte ring, not a `vt100` screen
  model, is the right first step.
- **Device store schema:** `<data_dir>/remote/devices.json`, a JSON array of
  camelCase `DeviceRecord`s (`deviceId`, `name`, `platform`, `publicKey`,
  `createdAt`, `lastSeenAt`, `pushToken?`, `pushEnvironment?`), written
  atomically, corrupt entries skipped per record. No permission field. Adding an
  optional `scopes` field with a default is a one-line serde change and needs no
  migration step (`remote/auth.rs:109-133, 187-190, 351-416`).
- **Docker/Podman on a Linux host:** the module has no platform gates, mounts
  three host paths at identical container paths, and carries
  `// TODO(linux-host): UID mapping before supporting Linux hosts`
  (`sandbox/docker/engine/mod.rs:53`). On macOS Docker Desktop maps ownership
  transparently; on Linux the container's UID must match the host user or the
  checkout comes back root-owned. This is real Phase 3 work, not a check.
- **Engine code outside the four dirs that needs Tauri:** `rpc/approval.rs` and
  `rpc/git/mod.rs:57` (`GitDispatcher.approval: Option<(AppHandle, String)>`).
  `sandbox/`, `workspace/`, `git/`, `database/`, `agent/`, `codegraph/`,
  `github/` have zero Tauri references.
- **Provider login headless:** `commands/provider_login.rs` is a Tauri command
  that streams a PTY to the webview; the CLI opens the browser itself via the
  login-shell `BROWSER`. On a headless box the answer is `ssh -t host claude
  login`, as the doc assumed. Both GitHub and Google OAuth in `oauth.rs` are
  device flows with no loopback listener, so `fletch-host github login` is
  straightforward.

### 1.3 Facts the original missed that change the plan

- **Agent IDs are place names from a ~300-entry pool, recycled after archive**
  (`workspace/agents.rs:65-67`, `names.rs`). Two hosts *will* both have an agent
  called `fuji`. Environment-scoped IDs in the client store (Phase 6) are a
  correctness requirement from the first multi-host build, not a later cleanup.
- **The desktop does not register `fletch://`.** Only the mobile package does
  (`mobile/src-tauri/tauri.conf.json:28-30`). The desktop only mints the link.
  Phase 6 needs `tauri-plugin-deep-link` on desktop, as the doc says, but it is
  new work, not a port.
- **`bin_resolve.rs:185` hardcodes `/bin/zsh`** for the login-shell PATH
  fallback, and `common_bin_paths` has no Linuxbrew path. On most Linux hosts the
  fallback silently no-ops. Phase 3 must use `$SHELL` or `sh -lc`.
- **Release is macOS-only, universal, `workflow_dispatch`**
  (`.github/workflows/release.yml`). A Linux `fletch-host` artifact is a new
  job, not a target added to the existing one.
- **The relay proxies every byte through a Durable Object.** T3Code's relay
  deliberately does not: after bootstrap, clients talk to the environment's own
  tunnel hostname (`t3code/docs/internals/t3-connect.md`). Streaming PTY output
  through a DO (Phase 5) has a cost and CPU-time profile the doc does not
  discuss. See §4, decision 4.
- **Every PTY chunk already travels as one base64 Tauri event on the desktop**,
  so Phase 5's stream design benefits the local UI too if the local path uses the
  same transport (§4, decision 3).

### 1.4 Verified counts kept from the original

`supervisor/` 10 of 11 files, 88 mentions; `roadmap/` 5 of 19, 57; 212 unique
`#[tauri::command]` fns, all registered at `lib.rs:1885-2102`; 18 in `workflow/`
and 30 in `roadmap/`; 41 ops in `OPS` + `register_push` as a session op; 15
forwarded events; `OUTBOUND_BUFFER = 64`, `MAX_IN_FLIGHT = 8`, 4 MiB frames,
20 s ping, drop-socket-on-full at `server.rs:316-325`; `EVENT_BUFFER = 256`
(lag logged at debug in `server.rs:601`); relay `MAX_DEVICES = 8`, 100 msgs/10 s
device→host only, `1008` on excess; 41 frontend files import `@tauri-apps/*`
outside `src/api/`; `src/api/invoke.ts` is one line; one `WorkspaceSlice`; no
Cargo workspace, two packages each with `snow = "0.10"`; `secrets.rs` and
`keychain.rs` fall back off macOS; `data_dir()` needs no `AppHandle`;
`git_dist` has Linux x86_64 and aarch64 assets; GitHub push auth is a per-command
`extraheader`, PRs go over REST/GraphQL, no `gh` binary anywhere.

## 2. What T3Code does that Fletch should copy

T3Code is Node/Effect/Electron, so none of its code ports. Its *decisions* do.
Each is verified in the tree at `/tmp/t3code`.

1. **The renderer is a client, even locally.** The desktop bundles a server and
   the renderer reaches it over the same HTTP+WebSocket RPC a phone uses; the UI
   is served from a `t3code://` scheme, never by the backend
   (`docs/internals/remote.md` "Desktop without a local environment",
   `apps/web/src/environments/primary/target.ts`). One code path, guaranteed
   parity, and the "local environment off" switch is trivial because the
   renderer never assumed a local server.
2. **Environment identity is independent of the route.** A server keeps its ID
   across restarts and endpoint changes; saved connections are client-local
   (`docs/internals/remote.md`). Fletch already has this: the host ID *is* the
   host public key. Use it as the environment ID everywhere in the client.
3. **Scopes per RPC, delegated at pairing, never widened.** Eight scope strings
   (`orchestration:read`, `orchestration:operate`, `terminal:operate`,
   `review:write`, `access:read/write`, `relay:read/write`,
   `packages/contracts/src/auth.ts:81-88`); every WS method maps to one
   (`apps/server/src/auth/RpcAuthorization.ts`), and a method without a scope is
   a type error. Pairing links carry a scope set; exchanging one can only narrow
   it (`docs/internals/environment-auth.md`).
4. **Capabilities, not versions.** The environment descriptor carries
   `serverVersion` and a `capabilities` struct; clients gate UI on flags and
   treat their absence as "hide", never compare versions
   (`packages/contracts/src/environment.ts:82,185-188`,
   `docs/internals/overview.md` "Pull request linking compatibility").
5. **Terminals are subscriptions with a snapshot.** The server retains bounded
   history per terminal, 5 000 lines and 8 MiB, in 16 KiB chunks; `terminal.attach`
   is a stream RPC whose first event is a snapshot, then incremental output;
   clients keep a separate 512 KiB buffer; query/response traffic is stripped
   from retained history and the renderer detaches its PTY writer during replay
   so old device queries do not provoke junk at the prompt
   (`apps/server/src/terminal/Manager.ts:93-95`,
   `packages/contracts/src/terminal.ts:51,100-107,222-227`,
   `docs/internals/terminal-runtime.md`).
6. **One shared client runtime.** `packages/client-runtime` owns one connection
   per environment, a registry scoped by environment, one retry owner, and
   cached projections that survive disconnects (`docs/internals/connection-runtime.md`).
   Web, desktop renderer and mobile all consume it.
7. **SSH as a host bootstrap, not just a tunnel.** Desktop main runs a shell
   script over SSH that installs the pinned runtime into `~/.t3/runtime`, starts
   `serve --host 127.0.0.1` under `nohup`, records ownership, and forwards the
   port; a server it merely discovered is never stopped on disconnect
   (`packages/ssh/src/tunnel.ts:451-533,704,753`). This removes "SSH in and run
   commands" from the user's path entirely.
8. **Direct routes for heavy traffic, relay as broker.** T3 Connect brokers
   credentials and managed tunnels; application traffic never crosses the relay
   Worker. Tailscale is treated as "an endpoint for ordinary pairing", needing no
   separate environment type (`docs/internals/t3-connect.md`, `remote.md`).
9. **A launcher owns the service and updates**, with prepared/committed handoff
   and a SQLite snapshot for rollback (`docs/internals/server-updates.md`). Too
   much for Fletch's first cut, but the shape (stable launcher, child runtime,
   `t3 update`, `t3 service install`) is what `fletch-host` should grow into.
10. **Local environment toggle relaunches the app** and skips port selection,
    exposure and backends; nothing is deleted (`docs/internals/remote.md`).

Not worth copying now: cloud identity (Clerk), managed tunnels, load balancing
across machines, the device panel. Fletch's Noise-based dumb relay gives
stronger privacy than T3 Connect and should stay.

## 3. Refined plan

Order: **0 → 1 → 2 → 3 → 4 → 5 → 6 → 7**, with 7's cheap half pulled into 0.
Phases 0–3 deliver "phone controls a headless box" with no client changes.
Each phase is one or a few PRs; the split is noted where it matters. Every
phase keeps CI green (`cargo fmt`, `clippy -D warnings`, `cargo test`, Bun
lint/typecheck/build/test) and changes `docs/remote-protocol.md` before code.

### Phase 0: Protocol groundwork (ship in the next desktop release)

Cheap, additive, and it means every desktop shipped from now on is a forward
compatible host for the clients built in Phases 5–7.

1. `hello` and `pair` results gain
   `protocol: { version: 2, ops: string[], events: string[], features: string[] }`.
   `ops` is the device's allowlist (today: the 41 + `register_push`), `events`
   the 15 forwarded names, `features: []`. Clients treat a missing `protocol` as
   this exact set. Prologue stays `fletch-remote-v2`.
2. Add op `answer_publish_approval { id, approved }` to `OPS`, calling
   `rpc::approval::answer` (unknown ids already ignored, `approval.rs:111-115`).
   The event is already forwarded; the phone just cannot answer it today.
3. Mobile: render `publish:approval-requested` as an Approve/Deny card, and read
   `protocol.ops` to hide what a host lacks.
4. Protocol doc: new "Compatibility" section (version field, unknown-op means
   unsupported, no `appVersion` comparisons) and the new op row.

Acceptance: old phone ↔ new desktop and new phone ↔ old desktop both pair and
work; new phone approves a gated push on a new desktop.

### Phase 1: Decouple the engine from Tauri

Outcome: `supervisor`, `workflow`, `roadmap`, `rpc`, `remote` compile with no
`tauri::` import except in `#[tauri::command]` wrappers. Desktop behaviour
unchanged, no wire changes. Three PRs, each independently green:

**1a. `EventSink`** (`src-tauri/src/host/sink.rs`): trait
`emit_value(&self, event: &str, payload: Value)`; `TauriSink`, `BroadcastSink`,
`FanoutSink`, `NullSink`; a generic `emit<T: Serialize>` helper that
serialises to `Value` first so `serialize_bytes_b64` on `AgentOutputPayload`
survives. Convert the 28 `.emit(` sites in `supervisor/events.rs`,
`roadmap/client_events.rs`, `roadmap/drainer.rs`, `workflow/journal.rs`,
`rpc/approval.rs`. Convert `GitDispatcher.approval` to hold a `Sink`.
Event names and payloads stay byte-identical; assert with a test that
`BroadcastSink` receives `roadmap:item` after a roadmap write.

**1b. `EngineCtx`** (`host/ctx.rs`): `{ sink: Sink, db: roadmap::Db,
supervisor: OnceLock<Arc<Supervisor>>, workflows: OnceLock<Arc<WorkflowService>>,
focus: Box<dyn Fn() -> bool + Send + Sync> }`. Replace the 7 `try_state` calls
and the `get_webview_window("main")` focus check. Change the constructors
(`SupervisorDriver::new`, `WorkflowService::new`, `drainer::spawn`,
`merge_sweep::spawn`, `SupervisorDispatch::new`) and every `Supervisor` method
and free fn listed in §1.1 to take `&EngineCtx` / `Arc<EngineCtx>`. This is the
big mechanical PR; use a sub-agent for the rename and review only the public
signatures, `lib.rs` wiring and tests.

**1c. Runtime and taps.** Replace the 22 `tauri::async_runtime::spawn` with
`tokio::spawn` (Tauri's runtime *is* tokio, so behaviour is unchanged inside the
desktop). Rewire `remote/events.rs::install_taps` and `remote/push.rs::install_taps`
to subscribe to the `BroadcastSink` receiver instead of `listen_any`, filtering
by the unchanged `FORWARDED_EVENTS`. `lib.rs` builds
`FanoutSink[TauriSink, BroadcastSink]` and keeps every `app.manage(...)` so the
212 Tauri commands still read `tauri::State`.

Out of scope, stays on `AppHandle`: `dictation/`, `oauth.rs`,
`commands/provider_login.rs`, `commands/tooling.rs`, tray, `power.rs`, updater.

Acceptance: `grep -rl 'tauri::' src-tauri/src/{supervisor,workflow,roadmap,rpc,remote}`
returns only files that define `#[tauri::command]` fns plus `remote/dispatch.rs`
(dictation arms); phone still gets `agent:status` and push alerts; CI green.

### Phase 2: Split the crates

Outcome: `crates/fletch-proto` (wire protocol, no Tauri, no engine) and
`crates/fletch-core` (engine, no Tauri), consumed by `src-tauri` (desktop
shell), `crates/fletch-host` (Phase 3) and `mobile/src-tauri`.

1. Root `Cargo.toml` workspace: members `src-tauri`, `crates/*`. Leave
   `mobile/src-tauri` out for now (its lockfile and iOS toolchain are separate);
   it depends on `fletch-proto` by path. Revisit once the split is stable.
2. **`fletch-proto` first**, in one PR: merge `src-tauri/src/remote/secure.rs`
   and `mobile/src-tauri/src/remote/secure.rs` into one module exposing both
   `initiator` and `responder`, the chunk codec, key load/create with the atomic
   0600 write, `mobile/.../dial.rs`, the frame types, relay proof and mux
   framing (all five types), and the known-answer test against
   `relay/test-vector.json`. Both apps switch to it; a phone built against it
   pairs with a desktop built against it.
3. **`fletch-core` incrementally.** Move a module, leave `pub use
   fletch_core::x;` behind in `src-tauri`, keep CI green, repeat. Order:
   `database`, `workspace`, `git`, `github`, `agent`, `sandbox`, `codegraph`,
   `rpc`, `host`, `supervisor`, `workflow`, `roadmap`, `remote` (minus Tauri
   commands). The compiler is the checklist.
4. **Split commands out of engine dirs.** The 48 `#[tauri::command]` fns in
   `workflow/` and `roadmap/` become `pub async fn …_impl(ctx, args)` in core
   plus thin wrappers in `src-tauri/src/commands/`, copying the existing
   `commit_agent_impl` pattern. Phase 4 builds on these `_impl` fns, so do this
   here, not later.
5. Platform gates as code moves: `sandbox/seatbelt.rs` behind
   `cfg(target_os = "macos")` (today it only *works* there), `keychain.rs`
   likewise; `secrets.rs` already is. `dictation/`, `power.rs`, Swift bridge
   stay in `src-tauri`.

Acceptance: `cargo tree -p fletch-core -i tauri` and `-p fletch-proto -i tauri`
empty; `cargo test -p fletch-core -p fletch-proto` on `ubuntu-latest` in CI;
desktop and phone unchanged in behaviour.

### Phase 3: `fletch-host`, headless

Outcome: `fletch-host serve` runs agents and is controllable from the existing
phone over LAN or relay. **Target a headless Mac first, Linux second.** The Mac
path reuses `sandbox-exec`, Keychain fallback and everything the desktop
already runs, so it isolates the "no window" problems from the "different OS"
problems; a Mac mini in a closet is also a real use case.

1. `fletch_core::host::boot(data_dir, sink, focus) -> Engine` extracted from
   `lib.rs:1449-1870`, preserving the current order and including the steps the
   original doc omitted (§1.1): approval/publish/push prefs, docker/podman launch
   settings and version guards, `git_dist::init` + startup, token seeding,
   `workspace::migrate_default_checkouts_root`, `rpc::sweep_orphan_mailboxes`,
   `Supervisor::new`, `WorkflowService::new` + `resume_active_runs`, drainer,
   merge sweep, `rehydrate_pending_messages`, orphan sweeps, `RemoteState::new`
   + taps + relay + autostart. Desktop `setup` calls `boot`, then does telemetry,
   tray, dictation, whisper, managed UI state. The existing SIGINT/SIGTERM
   handler (`lib.rs:1842-1865`) moves into `boot` and serves both binaries.
2. `crates/fletch-host/src/main.rs` (clap):
   `serve [--data-dir] [--port 47285] [--relay URL|--no-relay] [--name]`,
   `pair`, `devices list|revoke <id>`, `status`, `approvals list`,
   `approve <id> [--deny]`, `github login`. Everything but `serve` talks to the
   running server over a Unix socket `<data_dir>/host.sock` (mode 0600) with the
   same JSON envelope as the wire protocol, unauthenticated because the socket
   permission is the authentication. `pair` prints the `fletch://pair?…` URL,
   `expires_at`, and a terminal QR (`qrcode` crate).
3. Data dir: macOS keeps `data_dir()` as is; Linux uses
   `$XDG_DATA_HOME/fletch` via `dirs::data_dir()`, never `BUNDLE_ID`.
4. Sandbox default off macOS: no stored setting → Docker if
   `docker::availability()` is `Available`, else Podman, else refuse to start.
   **Fix the UID mapping** (`sandbox/docker/engine/mod.rs:53`): run the
   container as the host UID:GID (`--user`) or use Podman's `--userns=keep-id`.
   Acceptance test: files an agent writes are owned by the service user.
5. Linux shell: replace the hardcoded `/bin/zsh` in `bin_resolve.rs:185` with
   `$SHELL` falling back to `sh -lc`; add `/home/linuxbrew/.linuxbrew/bin` to
   `common_bin_paths`.
6. `fletch-host github login` runs the existing device flow (`oauth.rs`) and
   prints code + URL. Provider CLIs are logged in over SSH; document it.
7. Packaging: `packaging/fletch-host.service` (systemd user unit,
   `Restart=on-failure`) plus `loginctl enable-linger` docs; a launchd plist for
   macOS. Release: new job in `release.yml` building `fletch-host` for
   `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`.
8. Headless approval: with no client connected, a gated publish waits
   `wait_secs()` then refuses. `fletch-host approve` and the Phase 0 op are the
   two ways to answer; the README says so.

Acceptance: (a) Mac mini, `fletch-host serve` + `pair`, phone spawns an agent,
watches `agent:event`, approves a push, opens a PR. (b) Same on Ubuntu with
Docker. (c) Same through `wss://relay.fletch.sh`. (d) `kill -TERM` mid-run,
restart, run resumes.

### Phase 4: One op registry with scopes

Outcome: each engine operation is defined once with a required scope, reachable
as a Tauri command and as a remote op.

1. `fletch-core::ops`: `OpDef { name, scope, handler: fn(Arc<EngineCtx>, Value) -> BoxFuture<Result<Value, String>> }`
   collected in one `static OPS: &[OpDef]` (one file, no `inventory`). Scopes
   are **strings in a small fixed set**, stored on the device as
   `scopes: Vec<String>`: `agents` (today's phone surface), `files:write`,
   `terminal` (raw PTY, shells, run scripts), `workflows` (wf + roadmap),
   `settings`, `admin` (pairing, revoke, relay). Presets on the pairing UI:
   "Phone" = `[agents]`, "Full" = all but `admin`, "Admin" = all. A string list
   costs the same as an enum today and avoids a second format change later,
   which is the lesson from T3Code's scope table.
2. `DeviceRecord.scopes: Option<Vec<String>>`, `None` read as `[agents]`.
   `begin_pairing(scopes)` mints a token bound to a scope set; `pair` grants at
   most that set. Device store already skips unknown fields, so old files load.
3. Generic Tauri command `op(name, args)` looking up `OPS`; keep named commands
   as wrappers until the frontend has moved (Phase 6). `SupervisorDispatch::dispatch`
   becomes lookup + scope check; `"forbidden"` joins the reserved errors. Delete
   the hand-written `match` after a parity test proves every old `OPS` name
   resolves with scope `agents` and returns the same result through both paths.
4. Events get scopes: `(name, scope)` pairs replace `FORWARDED_EVENTS`.
   `agents` keeps today's 15; `workflows` adds `wf:*` and the 13 `roadmap:*`;
   `terminal` adds `run:state`, `run:port`; `settings` adds
   `session:sync-health`, `docker:build-progress`. PTY streams are Phase 5.
5. Migrate by domain, matching the 18 files in `src/api/domains/`. Desktop-only
   commands (dictation model download, updater, window, OAuth browser open,
   provider-login PTY) stay plain Tauri commands outside `OPS`.
6. `hello.protocol.ops/events` (Phase 0) are now generated from the table for
   the device's scopes. Generate the protocol doc's Operations and Events tables
   from the same table; CI fails on `git diff --exit-code`.

Acceptance: parity test; an `[agents]` device calling a `terminal` op gets
`forbidden`; `#[tauri::command]` count falls per PR.

### Phase 5: Terminal streams and flow control

Outcome: a `terminal`-scoped client watches and types into agent PTYs, shells
and run scripts on a remote host without disconnects.

1. **Host-side retained output per PTY**, a byte ring with a sequence counter,
   default 1 MiB per agent PTY and shell, matching the frontend's 256 KiB in
   spirit and T3Code's 8 MiB cap in shape. This replaces nothing on the desktop
   yet; it adds the snapshot source. Do **not** start with a `vt100` screen
   model: replaying raw bytes is what the desktop already does, T3Code does the
   same with a bounded history, and a screen model would be a second terminal
   emulator to keep correct. Strip query/response sequences on replay and have
   the client detach its PTY writer while replaying, as T3Code does.
2. Ops `stream_open { kind: agent|shell|run, id } -> { streamId, seq, snapshot }`
   and `stream_close`; the host sends bytes only for streams a connection
   opened. Run sessions already have their 5 MiB log; `stream_open` for `run`
   returns it.
3. Binary plaintext frame inside the Noise channel:
   `0x10 || streamId u32 || seq u64 || bytes`. Update the "Envelope" section
   first. JSON frames keep working unchanged, so old phones see no difference.
4. Per-stream bounded queue in `server.rs` separate from the 64-frame control
   queue; on overflow drop the stream's bytes and send
   `stream_reset { streamId, seq, snapshot }`, never close the socket. Coalesce
   output to 16 ms or 32 KiB per frame.
5. Client: batch keystrokes into one `stream_input` per 30 ms, debounce resize
   to 100 ms.
6. Relay: rate-limit device→host by bytes per window instead of messages, and
   raise the DO message cap only if measurement demands it. Note in
   `relay/README.md`; self-hosters redeploy. Measure DO CPU and cost with `cat`
   of a 50 MB file before deciding how much PTY traffic the public relay should
   carry (decision 4).

Acceptance: 50 MB `cat` over LAN and relay stays connected and ends in the
correct final screen; 60 s of continuous typing over the relay, no `1008`;
existing phones see identical traffic.

### Phase 6: Desktop as a client of many hosts

Outcome: the desktop lists environments ("This Mac" plus paired hosts), shows
each host's agents, drives any of them with the full UI, and can run with its
local engine off. Copy T3Code's shape: one shared client runtime, one
environment registry, the renderer talks to every environment the same way.

1. **Shared TS client runtime.** Move `mobile/src/remote/{client,types,backoff,candidates,pairing}.ts`
   to `packages/remote-client/` and add an **environment registry**: one
   `ProtocolClient` per host ID, one retry owner, connection state per
   environment, cached snapshot per environment. Mobile adopts it and gets
   multi-host for free; its store and persistence move from "the last host" to
   a keyed map.
2. **Multi-connection dialer in Rust** on `fletch-proto`: port the four mobile
   commands into `src-tauri/src/client/` with `HashMap<HostId, Connection>`,
   `hostId` on every command and event, "newest supersedes" per host. Mobile
   switches to the same module.
3. Desktop device identity at `<data_dir>/remote/device_key`, separate from
   `host_key`, same load-or-create rule. Register `fletch://` on desktop with
   `tauri-plugin-deep-link`. Settings → Remote → "Add host" accepts a pasted
   link or a scanned QR, requesting the "Full" preset.
4. **Transport interface.** `src/api/invoke.ts` and `events.ts` become a
   `Transport { call(op, args), on(event, cb) }` with `LocalTransport` (Tauri
   generic `op` command + `listen`) and `RemoteTransport` (`ProtocolClient`).
   Every `src/api/domains/*.ts` call takes the transport of the environment it
   acts on. Route the 5 raw `listen` files and 2 raw `invoke` files through it.
5. **Environment-scoped store:** `Record<EnvironmentId, WorkspaceSlice>` plus
   `activeEnvironmentId`, where the local engine's ID is its own `host_key`
   public key. Every selector, route and PTY buffer key that takes an agent ID
   takes `(environmentId, agentId)`, because agent names collide across hosts
   (§1.3).
6. Remote-aware pickers: the 7 `plugin-dialog` files get a host-side browser on
   `list_dir` and upload through `attachment_*` when the environment is remote.
7. Local engine off switch: a desktop setting that makes `setup` skip `boot`;
   changing it relaunches, nothing is deleted.
8. Optional but high leverage: **"Add host over SSH"**. Desktop runs a bootstrap
   script over the user's `ssh` (download the pinned `fletch-host` release into
   `~/.local/share/fletch/runtime/<version>`, `fletch-host serve` under
   `nohup`/systemd, `fletch-host pair --json`), then pairs over LAN or relay.
   Never stops a host it did not start. This is T3Code's `packages/ssh` idea and
   removes every manual step from Phase 3's acceptance test.

Acceptance: Desktop A (engine off) paired to Desktop B and a Linux
`fletch-host`: project, agent, workflow, tool-use approval, shell on each, from
A. Revoking A on B shows `4003` for B only. Single-machine users see one entry
in the switcher and nothing else changes.

### Phase 7: Compatibility and updates

1. Phase 0 already put `protocol` on the wire and Phase 4 generates it. Add
   `features` flags as they land: `streams`, `binaryFrames`, `githubConnected`.
2. Clients gate on `ops`/`features` membership only; `unknown op` is "not
   supported", never a transport failure.
3. Version skew notice in the environment status when `host.appVersion` is
   older than the client, linking to the release page.
4. `fletch-host update [version]`: download the matching release, verify
   minisign, swap a symlink, restart the service. Migrations are forward-only
   (`rusqlite_migration` `M::up` only, `database/connection.rs:79`), so copy the
   SQLite file, WAL and shm before restarting and document manual rollback.
   T3Code's prepared/committed handoff is the target shape once this bites.

## 4. Decisions for Alex

1. **Headless Mac before Linux in Phase 3.** Recommended: yes. It separates
   "no window" bugs from "different OS" bugs and is itself useful.
2. **Local calls through the registry.** Decided 2026-09-18: **no.** The local
   path stays on Tauri `invoke` and Tauri events, exactly as today. See §5.1.
   The registry (when it comes) is a host-side concern; the frontend's
   `LocalTransport` is a direct wrapper over `invoke`/`listen` with no extra
   hop, no handshake and no serialisation beyond what Tauri already does.
3. **Scopes as a string list, not an enum.** Recommended: list (see Phase 4).
4. **PTY traffic through the public relay.** Recommended: keep the relay as
   fallback, prefer direct routes (LAN, Tailscale address, SSH port-forward from
   Phase 6.8) for `terminal`-scoped clients, and switch the relay's device→host
   limit to bytes. Decide the byte budget after the Phase 5 measurement. A
   hostile or costly relay should degrade streams, never control.
5. **"Full" pairing second factor.** A `terminal`-scoped device can run shells.
   Recommended: require confirming the pairing on the host (`fletch-host pair
   --confirm`, or a desktop dialog) for any preset that includes `terminal` or
   `files:write`.
6. **Stop after Phase 3?** Phases 0–3 give "phone controls a headless box" with
   no client rewrite. If nobody asks for desktop-as-client, stop there; Phases
   4–7 are the desktop story.

## 5. Scope decision (2026-09-18)

Alex's two goals: (1) `fletch-core` runs headless on a Linux box from the CLI
and behaves like the desktop engine; (2) spawn an agent from the laptop onto a
pre-configured cloud host, close the laptop, the agent keeps running, check in
and spawn from the phone. Minimal but solid; no SSH bootstrap or other
conveniences in v1.

Hard constraint: **the local desktop must feel exactly as it does today.** No
pairing, no handshake, no loopback socket, no perceptible delay for the local
engine, and **no current desktop feature is cut**. Everything in §5.3 is a
*remote-parity* gap only: it lists what a client cannot yet do *against a remote
host*, never something the desktop loses locally.

### 5.1 Local path: how host and client stay one app

- The engine runs in the desktop process, booted in Tauri `setup` as today,
  same database, same data dir. `boot()` is the same function the headless
  binary calls; the desktop just calls it in-process. Decided 2026-09-18 over a
  sidecar process (T3Code's model): a sidecar would force every command and all
  PTY output over a socket on day one, making the op registry and PTY streams
  prerequisites for v1 and splitting dictation, provider login, the activity
  monitor and the `db_*` bridge across two processes. The `EngineCtx` /
  `EventSink` boundary keeps the sidecar a later swap (deferred item 15a).
- The frontend's `Transport` has two implementations. `LocalTransport` wraps
  Tauri `invoke` and `listen` directly. It is the current code with one
  indirection and adds no serialisation, no async hop, no allowlist check: all
  212 commands stay reachable locally. `RemoteTransport` wraps the shared
  `ProtocolClient`. Nothing local ever touches Noise, the WebSocket server or
  `devices.json`.
- Events reach the webview through `TauriSink`, the same `app.emit` as today.
  `BroadcastSink` sits beside it in a `FanoutSink`; with no remote receivers a
  `broadcast::Sender::send` returns immediately. One `serde_json::to_value` per
  event is the only added work, and Tauri already serialises the payload.
- The local environment has a fixed client-side ID (`"local"`, following
  T3Code's `PRIMARY_LOCAL_ENVIRONMENT_ID`), present in the store from the first
  render in state `connected`. It never depends on `host_key` or on remote
  access being enabled. Host public keys identify *remote* environments only.
- Agent references become `(environmentId, agentId)` internally because agent
  names collide across hosts; for the local environment this is invisible.
- Capability gating (`protocol.ops`, `protocol.events`) applies to remote
  environments only. The local environment is never gated.

### 5.2 v1 scope

Phases **0, 1, 2, 3** as written in §3, Linux as the Phase 3 target with a
headless macOS run as a free smoke test, then a **reduced Phase 6**:

1. Shared TS protocol client package (moved from `mobile/src/remote/`), with a
   per-environment registry; mobile adopts it.
2. Desktop Rust dialer on `fletch-proto`, keyed by host ID from day one, plus a
   desktop device key.
3. Settings → Remote → "Add host": paste a pairing link. `admin`-free, phone
   allowlist.
4. Environment-keyed store and `Transport` routing (`invoke` → `transport(env).call`,
   `listen` → `env.on`), including the 5 raw `listen` and 2 raw `invoke` files.
5. Host picker on the spawn flow; remote agents use the structured view (the
   host already forces it for phone spawns); refetch on reconnect as the phone
   does.
6. Remote-environment UI degrades by hiding or disabling what `protocol.ops`
   lacks, with a reason. Local UI is untouched.

Connectivity: open TCP 47285 on the cloud box and dial directly (Noise gives
confidentiality and mutual authentication); relay as the fallback for the phone.

Projects on the cloud host are added with the host CLI (`fletch-host project
add <path>` / `clone <spec> <dest>`, wrapping `add_workspace_repo` /
`clone_repo`) or from the phone's existing flow.

### 5.3 Deferred, ordered by impact ÷ effort (highest first)

Effort: S=1, M=2, L=3, XL=4. Impact: 1 (nice) to 5 (unlocks a core use case).
Every row is a remote-parity gap; local behaviour is unaffected.

| # | Item | Effort | Impact | Index | Depends on | What it unlocks |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | Second factor for pairings that include `terminal`/`files:write` (confirm on host) | S | 3 | 3.0 | 6 | Safe shell access from remote clients |
| 2 | Expose `wf_*` and `roadmap_*` ops and their 16 events remotely (table rows once Phase 2 `_impl` split exists) | M | 5 | 2.5 | Phase 2 | Autopilot and workflows on the cloud box driven from laptop or phone |
| 3 | Version-skew notice in environment status | S | 2 | 2.0 | Phase 0 | Tells the user why a feature is hidden |
| 4 | `fletch://` deep link on desktop (`tauri-plugin-deep-link`) | S | 2 | 2.0 | – | Click a pairing link instead of pasting |
| 5 | Local engine off switch (skip `boot`, relaunch) | S | 2 | 2.0 | Phase 6 | Laptop as pure client, saves battery and RAM |
| 6 | Tailscale as the pairing address (docs only; `addr` may be a tailnet IP) | S | 2 | 2.0 | – | Private route without opening a port |
| 7 | Headless macOS `launchd` plist | S | 2 | 2.0 | Phase 3 | Mac mini as a host service |
| 8 | Relay device→host limit by bytes instead of messages | S | 2 | 2.0 | 12 | Keystroke streams through the relay |
| 9 | Remote-aware pickers on desktop (`list_dir` browser, `attachment_*` upload) for the 7 `plugin-dialog` files | M | 3 | 1.5 | Phase 6 | Add projects and attach files to a remote host from the desktop |
| 10 | `fletch-host update [version]` (download, verify, swap, restart; SQLite backup first) | M | 3 | 1.5 | Phase 3 | Upgrade a host without SSH gymnastics |
| 11 | `admin` op to start the GitHub device flow from a client and return the code | M | 3 | 1.5 | 13 | GitHub login on a host without a shell |
| 12 | Mobile multi-host UI (store map, host switcher) on top of the shared registry | M | 3 | 1.5 | Phase 6 step 1 | Phone controls several hosts |
| 13 | Op registry with per-device `scopes` (Phase 4) | L | 3 | 1.0 | Phase 2 | Graduated permissions; prerequisite for terminal ops |
| 14 | PTY streams (Phase 5): host-side retained buffer, `stream_open/close`, binary frames, coalescing | XL | 5 | 1.25 | 13 | Native-view agents, side shells and run scripts on remote hosts |
| 15 | Generated Operations/Events tables in `docs/remote-protocol.md`, CI diff check | M | 2 | 1.0 | 13 | Doc cannot drift from code |
| 15a | Local engine as a sidecar process (desktop spawns `fletch-host`, renderer connects over a Unix socket, T3Code's model) | L | 3 | 1.0 | 13, 14 | Agents survive a webview crash or app restart; one code path for local and remote. Not before 13 and 14: without them all 212 commands and PTY output would have to cross the socket on day one |
| 16 | "Add host over SSH" bootstrap (install runtime, start service, pair) | L | 3 | 1.0 | Phase 3, Phase 6 | Zero-touch cloud host setup |
| 17 | Stable launcher with prepared/committed handoff and DB rollback | L | 2 | 0.67 | 10 | Safe unattended upgrades |
| 18 | Load balancing new agents across hosts | L | 1 | 0.33 | 12 | Convenience |
| 19 | Windows host | XL | 1 | 0.25 | Phase 3 | – |

Rows 13 and 14 are ordered by dependency rather than strictly by index: 14
cannot start before 13.

## 6. Progress checklist

Kept current as PRs open and merge. Status legend: `[x]` merged to `main`,
`[o]` PR open, `[~]` in progress on a branch, `[ ]` not started. Last update
2026-09-19, late: all v1 code merged (#749–#768). Three hardening PRs open
(#770, #771, #773). Left in v1: the three verification lines (3c, 3e, manual
pass), which need a real machine and a phone.

### 6.1 v1 scope (§5.2)

**Phase 0: protocol groundwork**
- [x] `protocol { version, ops, events, features }` on `pair`/`hello`; op `answer_publish_approval`; mobile approve/deny card; protocol doc "Compatibility" section; this plan committed. PR #751.

**Phase 1: decouple the engine from Tauri**
- [x] 1a `EventSink`, `host::emit`, 28 emit sites converted. PR #750.
- [x] 1b `EngineCtx`, `TauriSink`, 7 service-locator lookups and the focus check removed, all `Supervisor` signatures. PR #754.
- [x] 1c `host::runtime` (global tokio handle; 22 spawn sites), `BroadcastSink`, `FanoutSink`, remote taps off the Tauri bus. PR #757.

**Phase 2: crates**
- [x] 2a `crates/fletch-proto` (keys, noise, dial, relay framing, test vector); both apps depend by path; no Cargo workspace; CI gate. PR #752.
- [x] 2b `crates/fletch-core`: 47 engine modules moved behind `pub use` shims; dictation ops via `SupervisorDispatch::with_dictation` + `BootConfig::dictation` hook; `cargo tree -i tauri` empty; CI gate; tests 1651 core + 68 shell = 1715 (baseline exact). PR #764. Precedes 3b: a binary depending on the Tauri app crate would pull webkit into the Linux build.
- [x] 2c `_impl` split of the 48 `#[tauri::command]` fns in `workflow/` and `roadmap/` (+18 the remote dispatcher needs); 212 handler names unchanged. PR #764.

**Phase 3: fletch-host**
- [x] 3a `host::boot(BootConfig) -> Engine` extracted from Tauri `setup`; dictation arms read `ctx.db` instead of an `AppHandle`; `BootConfig` hooks for DB recovery dialog, activity monitor, exit path. PR #762.
- [x] 3b `crates/fletch-host` binary: `serve [--data-dir] [--port] [--relay|--no-relay] [--name]` (`RemoteBoot::Headless`, `NullSink` host sink, settings-table secrets, Docker/Podman default off macOS), `pair` (URL + QR), `devices list|revoke`, `status`, `approvals list`, `approve <id> [--deny]`, `github login`, `project add|clone`; admin Unix socket `<data_dir>/host.sock` 0600 with the wire envelope; default data dir `dirs::data_dir()/fletch-host`; e2e test pairs a `fletch-proto` client against a booted headless engine. PR #766.
- [ ] 3c headless macOS smoke test (free checkpoint): `fletch-host serve` + phone pair on this Mac.
- [o] 3d Linux: Docker UID mapping (`--user uid:gid` + 1777 tmpfs home, Linux only, skipped for rootless Docker; Podman untouched on purpose) with an opt-in real-Docker acceptance test; login shell from `$SHELL` → `/bin/zsh` → `/bin/sh`; Linuxbrew path; host data dir `dev` split and its own checkout/RPC roots under that data dir; QR for `pair` (feature `qr`, default on); `packaging/fletch-host.service` + `com.fletch.host.plist`; release job `host` for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` (arm runner), `aarch64-apple-darwin`. Docker/Podman default when no setting landed in #766. Review fixes in the same PR: owned-root sweep via `StateRoots`, state roots per data dir, one product version checked by `scripts/check-version.sh`, rootless probe cached only on success. PR #767.
- [ ] 3e Acceptance: Ubuntu + Docker + `claude` logged in over SSH; phone pairs (LAN and relay), spawns, approves a push, opens a PR; `kill -TERM` mid-run then restart resumes.

**Hardening after v1 (gaps the merged PRs documented)**
- [x] Follow-ups: `merge_pr`/`discard_agent`/`restore_agent`/`get_all_git_meta`/`get_pr_threads` on the wire or gated; this Mac's name at pairing; sandbox OAuth commands pinned local; `publish:approval-resolved` event; engine facts in `fletch-host status`. PR #768.
- [x] Git panel working-tree ops on the wire (commit, push, PR, update-branch, conflicts, checks, comments) gated by op name; `delete_branch_agent` withheld on policy; gate enforced in the dispatch, not only the button. PR #770.
- [x] Seatbelt denies the headless host's `--data-dir` (DB, host key, devices) and re-allows its own `workspaces`/`rpc` roots and read-only `git-dist`; desktop profile byte-identical. Acceptance test passed under real `sandbox-exec`. PR #771.
- [x] The ignored `seatbelt_denies_appsupport_auto_exec` test was vacuous (unquoted path with a space in `sh -c`); quoted. PR #773.
- [o] Linux hardening (2026-09-25): a uid-mapped launch binds a generated two-line `/etc/passwd` (`<writable_root>/.fletch-passwd`, read-only, `no-new-privileges`) so a mapped uid ≠ 1000 resolves for `os.userInfo()`/`whoami`; the Cursor image installs under `/opt/cursor-agent` instead of `/root` so it runs under `--user`. Unit gates green; the Docker-backed tests and test-plan rows 5.2/5.7 still need a machine with Docker (none on this Mac), ideally rootful Linux.
- [ ] Known small gaps, not started: desktop's own `git-dist` is read-denied inside the agent sandbox (pre-existing); `~/.fletch/tools` is shared by every engine on a machine (assessed 2026-09-25: only the codegraph bundle lives there, it is never installed on Linux, and the install is a staged rename, so two engines on one Mac can at worst fail one silent install that retries — left as is); `refresh_base_freshness` is silent remotely. (The missing gate reason on disabled Git panel buttons is in the polish PR below.)

**Desktop as client (reduced Phase 6)**
- [x] Shared TS protocol client moved to `src/remote/`. PR #749.
- [x] Environment-keyed store + `Transport` seam (`LocalTransport`, `RemoteTransport`, `invokeLocal`/`onLocal` for desktop-only domains; PTY buffers env-keyed). PR #753.
- [x] Shared Rust dialer `fletch_proto::client::Dialer` (many connections, keyed by connection id); desktop `remote_connect/send/close/device_public_key`; desktop device key. PR #756.
- [x] Paired hosts part a: `remote.hosts` setting, background connections after first paint, Settings → Remote control → Paired hosts (paste link, state dot, Forget). PR #758.
- [x] Paired hosts part b: switcher in the sidebar header (only when a host is saved), `switchEnvironment` (stash/restore workspace-shaped state + selected agent, detach/re-attach engine listeners), per-slice agent-id collision table, capability gating from `protocol` (add project, shells, run, workflows, roadmap, native view, fork; autopilot local-only), reconnect refetch, `retrying` on the entry. PR #763.
- [ ] Manual pass (all client PRs merged): local app unchanged with no hosts; device key created lazily and `host_key` untouched; native-view replay after env-keyed PTY buffers; pair a phone against a desktop built on `fletch-proto`.

### 6.2 Deferred (§5.3)

- [x] 7 Headless macOS `launchd` plist: shipped as `packaging/com.fletch.host.plist` in PR #767.

One "remote polish" PR (`feat/remote-polish`) folds the next four deferred
items plus one hardening gap together. Tailscale (item 6) is dropped on
purpose: the plan is an own secure pairing procedure later, not a tailnet.

- [o] 2 `wf_*` / `roadmap_*` ops remotely: all 19 `wf_*` (incl. `wf_run_agents`)
  and 31 `roadmap_*` commands on `dispatch::OPS`, their 16 events on the
  forwarded list, the `workflows` gate opened by `wf_list_runs` and the
  `roadmap` gate re-pointed at `roadmap_create_item` (a board *write*, since
  `roadmap_list_items` has been on the wire since the phone's planning chat),
  and autopilot moved from a hard `kind === "remote"` rule to a
  `GATES.autopilot` row with `op: null` (its opt-outs are this Mac's local
  tables). Nothing withheld inside the two families; what stays off is what
  those flows borrow from other families (`list_repo_tree`/`list_repo_prs`,
  `run_verification`, `fork_agent`), recorded in
  `dispatch::WITHHELD_WF_ROADMAP_OPS`. No protocol version bump: additive.
- [o] 3 Version skew notice: host `appVersion` kept in memory on the
  environment entry; `closedGates`/`hostSkew` in `src/store/capabilities.ts`
  derive "N features unavailable on this host" from op membership only; shown
  on the paired-host row and the switcher entry with a tooltip listing each
  reason and both versions. Gate reasons now render as `title` + the panel's
  notice line on every disabled Git panel action (closes the hardening gap).
- [o] `fletch-host service install|uninstall`: renders `packaging/*.service`
  / `*.plist` (now templates, `include_str!`-ed, one copy) with the resolved
  binary, data dir and serve flags; systemd user unit by default, `--system`
  with `User=`; launchd bootstrap on macOS; idempotent; prints every file and
  command.
- [o] 10 `fletch-host update [version] [--check]`: GitHub release asset for
  the compile-time target; sha256 AND minisign `.sig` against the desktop
  updater's public key (test pins it to `tauri.conf.json`); fails closed on a
  missing or bad signature; SQLite copied to `<data_dir>/backups/` first;
  atomic rename over the running binary; restart via the installed unit, else
  a message. The release `host` job now signs the tarball with the existing
  updater key and uploads `.sig`.

### 6.3 Closing the remaining gaps (2026-09-21)

After the polish PRs merged, an audit found four gaps: no provider CLI login
on a headless host, projects only addable where someone has local access, no
scope at pairing, and plaintext secrets on a host. Each was verified against
the code and sliced into six PRs; the verdicts, scopes and order are in
`multi-host-gaps-plan.md`, whose per-PR acceptance is what to run for these six.
The manual test matrix for the surface underneath them (#749–#784) is in
`multi-host-test-plan.md`.

- [x] Gap 4: GitHub token from `FLETCH_GITHUB_TOKEN` or a systemd credential, never persisted. PR #785.
- [x] Gap 2a: host folder browser over `list_dir` on the desktop (native picker stays local); `is_repo`/`truncated` on `list_dir`; per-flow gates. PR #786.
- [x] Gap 2b: project settings ops on the wire (`create_repo`, rename, delete, attach, detach, relocate, label, remove). PR #788 on #786.
- [x] Gap 1a: `fletch-host provider status|login <id>`; bounded `--version` probes everywhere. PR #787.
- [x] Gap 1b: `host_providers` op; clients grey providers the host lacks and quote the fix. PR #789 on #787.
- [o] Gap 3: device scopes at pairing (Full / Control), per-device `protocol.ops`, `forbidden`. PR #791.

Nothing else from the deferred table has been started. Items 13 and 14
(scopes, PTY streams) remain the big parity gap: remote agents are
structured-view only, with no side shells or Run panel on the host.

### 6.4 How this work is being run (for whoever picks it up)

- One PR per checklist line, built by an Opus sub-agent in a git worktree under
  `.claude/worktrees/`, all sharing `CARGO_TARGET_DIR=<checkout>/src-tauri/target`.
  Agents commit, never push; the coordinator reviews the diff and any reported
  deviation against the code, then checks the branch out in the main checkout
  and opens the PR through the Fletch host RPC (`open_pr`, `args.base` for
  stacked PRs).
- Stacked PRs target their parent branch; merge bottom-up with merge commits
  or rebase-merge, not squash. After a restart the host refuses `git_push`
  with `force`, so a branch that needs `main` is updated by merging `main`
  into it (the app's own "update branch" flow), not by rebasing.
- Every PR must pass locally: `cargo fmt --check`, `cargo clippy --all-targets
  -- -D warnings -A clippy::significant_drop_tightening`, `cargo test` for
  `src-tauri` and `crates/*`; `bun run lint|check|build|test` at the root and
  in `mobile/` when touched. CI on `ubuntu-latest` is the final gate.
- Deviations accepted so far, each verified in code: fallible `emit_value`
  (publish gate fails closed); `host::runtime::spawn` instead of bare
  `tokio::spawn` (half the spawn sites run on std threads or in `setup`);
  push triggers as a sink, not a broadcast subscriber (race with
  `drain_message_queue`); dialer keyed by connection id, not host id (the TS
  contract has no `hostId`); no Cargo workspace (path deps only).
