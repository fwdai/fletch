# Multi-host: manual test plan

2026-09-20. Covers PRs #749–#784 (Phases 0–3, the reduced Phase 6, hardening,
the remote-polish PRs, and the mobile follow-ups). Every line is grounded in
the code as merged, not in `multi-host-plan.md`. File refs are where the
behaviour lives, so a failure has a starting point.

Legend: **P0** must pass before a release, **P1** should pass, **P2** nice to
confirm. "Known gap" means the code behaves that way on purpose or the gap is
already recorded; check it does not get worse.

## 0. Automated coverage (run first, cheap)

Already green on this checkout on 2026-09-20: `bun run test` at the root
(135 files / 1439 tests) and in `mobile/` (17 / 173). CI runs the four Rust
crates on Ubuntu, so everything macOS-only never runs there. Run locally:

```sh
cargo test --manifest-path crates/fletch-core/Cargo.toml
cargo test --manifest-path crates/fletch-host/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
./scripts/check-version.sh

# macOS-only kernel acceptance tests, ignored on CI:
cargo test --manifest-path crates/fletch-core/Cargo.toml --lib -- --ignored --nocapture \
  seatbelt_denies_the_configured_data_dir \
  seatbelt_denies_appsupport_auto_exec \
  seatbelt_denies_writing_git_config \
  shutdown_kills_sandbox_exec_grandchild

# With Docker Desktop running (macOS shape: no --user mapping expected):
FLETCH_DOCKER_TESTS=1 cargo test --manifest-path crates/fletch-core/Cargo.toml -- --ignored docker_
```

`docker_run_as_host_user_writes_user_owned_files` only means something on a
Linux box (see §7).

## 1. P0 — local desktop, no hosts saved (the "feels exactly as today" rule)

The engine now boots through `fletch_core::host::boot` (`crates/fletch-core/src/host/boot.rs:233-666`)
with events fanned through `FanoutSink[TauriSink, BroadcastSink]`; 48 workflow
and roadmap commands were split into `_impl` + thin wrappers; PTY buffers are
keyed by environment. None of this should be visible.

| # | Check | How | Expect |
|---|---|---|---|
| 1.1 | Nothing new appears | Fresh launch, no paired hosts | No environment switcher in the sidebar header (`src/components/Sidebar/EnvironmentSwitcher/index.tsx:19-22` hides it under 2 envs). Settings › Remote control › Paired hosts reads "None yet." |
| 1.2 | Host identity untouched | `shasum ~/Library/Application\ Support/com.fletch.desktop[/dev]/remote/host_key` before and after the session | Unchanged. `remote/device_key` does **not** exist until you add a host (§4.1). |
| 1.3 | Boot order: resumed work | Quit the app with an active workflow run and a running agent; relaunch | Run resumes (`resume_active_runs`, boot.rs:505), the agent row is rehydrated, activity monitor works (Stop from the tray/menu still kills agents). |
| 1.4 | Boot: DB recovery hook | Temporarily corrupt/lock the SQLite file (or point at a file with garbage) | The rfd dialog with Move Aside / Reveal Logs / Quit appears (`src-tauri/src/lib.rs:199-218`), not a silent crash. |
| 1.5 | Events reach the webview | Spawn an agent; watch status, tool-use approvals, transcript, git state, PR badges, `verify:report` | Everything updates live. This is `TauriSink` + every converted `emit` site. |
| 1.6 | Native view replay | Agent in native view with lots of output; switch to another agent and back; also toggle Custom → Native | Scrollback replays (256 KiB ring now keyed `local:<agent>`, `src/pty/buffers.ts:28-29`). Side shell replay likewise. |
| 1.7 | Publish approval dialog | `publish_confirmation` on; agent pushes | Dialog appears; Approve/Deny works; a Deny refuses the push. **Changed behaviour:** if the webview cannot show it, the push now waits out `publish_approval_wait` (120 s) then refuses, instead of refusing instantly (`crates/fletch-core/src/rpc/approval.rs:132-178`, `host/sink.rs:79-85`). |
| 1.8 | Workflows locally | Launch, approve, reject, retry, cancel, resolve a conflict, delete a run; save/import/export a YAML definition | All work. These now go through `*_impl` (`crates/fletch-core/src/commands/`). |
| 1.9 | Roadmap locally | Create item, rank, hand off, review, hold/release item and project, proposals (accept/reject), brief proposal, delete | Same reason as 1.8; 31 commands split. |
| 1.10 | Autopilot locally | Enable on a project; wait for a pass | Runs. Autopilot is now a `GATES` row with `op: null`; local must be ungated (`src/store/capabilities.ts:73,145-150`). |
| 1.11 | Git panel locally | Every action: commit, push, open PR, update branch, resolve conflicts, fix checks, comments, pull, rebase, stash, discard, abort merge, delete branch, merge PR; also Cmd/Ctrl+Enter from the commit composer | No action disabled, no gate notice. The dispatch-level gate (`useGitActions.ts:130-134`) must be a no-op locally. |
| 1.12 | Seatbelt unchanged | From an agent shell: `touch ~/.gitconfig.x`, `ls "$HOME/Library/Application Support/com.fletch.desktop"`, `ls ~/.fletch/tools`, write in the checkout | Denied, denied, allowed, allowed. Profile text is meant to be byte-identical; only the empty-string case is unit-tested (`seatbelt.rs:200-210, 1796`). If `~/Library/Application Support` is a **symlink** on your machine, the host deny block would leak into the desktop profile; check `ls ~/.fletch/rpc/<agent>` still works either way. |
| 1.13 | Secrets on release build | GitHub connected → restart | Still connected (keychain path unchanged, `secrets.rs:54-55`). Debug builds always used the settings table. |
| 1.14 | Locked keychain at login | Add Fletch as a login item, log out/in | GitHub does not read as signed-out; the mirror retries in the background (boot.rs:714-739). |
| 1.15 | Local-only surfaces | Dictation, provider sign-in (Claude/Codex), model catalog, updater check, Docker Desktop start, Claude container auth | Unaffected; these are pinned to `invokeLocal`/`onLocal`. |
| 1.16 | Nested Fletch via Run panel | Run Fletch from inside a Fletch agent (sets `FLETCH_WORKSPACES_ROOT`/`FLETCH_RPC_ROOT`) | Inner engine is `Inherited` and does not sweep the outer engine's mailboxes (boot.rs:71-107, 479-486). |
| 1.17 | Remote control on desktop | Enable, set port, set a relay, generate a pairing link, disable | Listener now autostarts inside `boot`; a stored bad relay URL is a warning, not a failed launch (boot.rs:568-586). Pairing refused while not listening. |
| 1.18 | Telemetry (P2) | Agents resumed at launch | `agent_spawned` for resumed agents is dropped because telemetry inits after boot (lib.rs:1430). Decide whether you care. |

## 2. P0 — phone ↔ this Mac's desktop as host

| # | Check | How | Expect |
|---|---|---|---|
| 2.1 | Pair over LAN and over relay | Settings › Remote control › Pair a device; scan on the phone; repeat with relay configured and Wi-Fi off on the phone | Pairs. Pair screen title is "Pair with <ComputerName>" (`scutil --get ComputerName`, `remote/mod.rs:656-674`). Device row on the Mac shows the phone. |
| 2.2 | `protocol` descriptor | Debug log or the phone's host sheet | `hello`/`pair` carry `protocol { version: 2, ops (108), events (32), features: [] }`. |
| 2.3 | Spawn from the phone | New agent from the phone on a project | Lands on the desktop in the **structured (Custom) view** (`dispatch.rs:337`); events stream both ways; stop/resume/archive/restore/discard from the phone reflect on the desktop. |
| 2.4 | Publish approval from the phone | `publish_confirmation` on; agent pushes; answer on the phone; repeat answering on the Mac; repeat letting it expire | Card shows on the phone with Approve/Deny; whichever side answers closes it everywhere (`publish:approval-resolved`); expiry closes it. **Known gap:** a phone that connects after the request fired never sees the card (no list op). |
| 2.5 | Plan with PM | Home FAB → project → idea → Start planning; act on proposals: Add to backlog, Add & start agent, Discard; delete the chat (two taps) | Proposals arrive live (`roadmap:item`), each action moves the card; the planning chat is hidden from the desktop's normal agent list but visible on the phone's project screen. |
| 2.6 | Git from the phone | Commit, push, create PR, merge PR, pull/rebase/stash/discard/abort merge | Desktop Git panel reflects each via forwarded events (`agent:git-action`, `pr:state_changed`). |
| 2.7 | Revoke from the Mac | Settings › Remote control › device › revoke | Phone drops with 4003 "not paired any more", does not retry; re-pair works with a fresh link. |
| 2.8 | Push alerts | Desktop unfocused; agent finishes or needs approval | Push arrives (relay NOTIFY); focused window suppresses (`remote/push.rs:330-332`). |
| 2.9 | Live-turn replay (#783) | Start a long turn; background the phone or kill Wi-Fi for a minute; reopen the agent | Prompt bubble, then the missed stream, no duplicated frames; at turn end the log rebuilds from records. A "N earlier events … were not kept" notice needs >2000 events in one turn. |
| 2.10 | Dead socket after resume (#784) | Phone connected; suspend app several minutes or toggle the Mac's network; foreground | Within ~6 s the banner flips to reconnecting and the snapshot refreshes (`mobile/src/store/index.ts:544-558`). |
| 2.11 | Sub-agents on the phone (#779) | Claude agent that launches background Task/Agent tools; make one fail | Row reads "N sub-agents", strip above the transcript, "sub-agent failed" chip for 10 min. |
| 2.12 | Second host on the phone | Pair the phone with a second host (e.g. §5's `fletch-host`) | **Known gap:** the phone is single-host; the first host is silently replaced (`mobile/src/store/persist.ts:13-30`). Its device record stays on the old host until revoked. |
| 2.13 | Compat skew (P1) | If you have an older phone build: pair against this desktop; if you have an older desktop build: pair the new phone | Old phone ignores `protocol`; new phone against a pre-protocol host falls back to `V2_DEFAULT_OPS` and hides Plan/approve buttons. Nothing reconnect-loops on `unknown op`. |

## 3. P0 — desktop as a client of a host

Use either a second Mac running the desktop, or `fletch-host serve` on this
Mac (§5) as the host. Point the client at it from Settings › Remote control ›
Paired hosts.

| # | Check | How | Expect |
|---|---|---|---|
| 3.1 | Add host | Paste the `fletch://pair?…` link | Row goes connecting → connected; name is the host's own name; "vX.Y.Z" shown; `<data_dir>/remote/device_key` now exists, `host_key` hash unchanged (1.2). `remote.hosts` setting holds `{hostKey,name,addr,relay?,pairedAt}`. |
| 3.2 | Link refusals | Paste a non-Fletch URL; a link with `host=` removed; with `token=` removed; a link whose token expired; a link whose `host=` you edited | Each has its own message (`pairLink.ts:17-32`); host-key mismatch and bad token are **not** retried; nothing is saved on failure. |
| 3.3 | Skew line | Hover "N features unavailable on this host" on the row and in the switcher | Tooltip lists each closed gate with its reason plus "Host vA · this app vB". Against a current host expect 5: Terminals, Running the app, Native view, Forking, Deleting a branch (`capabilities.ts:191-213`). |
| 3.4 | Switch to the host | Sidebar header switcher → host | Projects/agents from the host; local UI state disappears. Switch back → **exact** local state: selected agent, drafts, right panel tab, history open, PR states (`environmentSwitch.ts:47-92`). |
| 3.5 | PTY buffers per env | Local agent in native view with output; switch to host; switch back | Native view replays the local scrollback (buffer key `local:<agent>` vs `<hostKey>:<agent>`). |
| 3.6 | Gates on the remote env | Look at every gated surface | Add project CTA disabled with tooltip; Home "Add a project" card hidden; **Run and Terminal tabs absent**; Workflows nav present and working (host exposes `wf_*`); Project settings gear present; roadmap pill enabled; autopilot does nothing; ViewToggle Native disabled with reason; ForkMenu gone; History restore works; Git panel merge/pull/rebase/stash/discard/abort work, delete-branch disabled with reason; Cmd+Enter on a gated action shows the notice line instead of running. |
| 3.7 | Drive an agent remotely | Spawn, send a message, answer tool use, stop, archive/restore, commit/push/PR/merge, resolve conflicts, update branch | All work end to end; the host's own UI (if a desktop) shows the same. |
| 3.8 | Workflows and roadmap remotely | Launch a workflow on the host from the client; approve/reject; roadmap board ops; autopilot stays off | Live via the 16 forwarded `wf:*`/`roadmap:*` events. `wf_run_agents`, `run_verification`-backed steps: verification is withheld remotely; note what a workflow that needs it does (expect a clear failure, not a hang). |
| 3.9 | Host goes away | Kill the host mid-use; restart it | Dot amber (retrying) with backoff up to 30 s; stale workspace stays on screen; `invoke`s fail quietly. On return: reconnect, `get_workspace` refetch, selected agent's transcript resyncs (mid-turn transcript preserved). If the host was never loaded, the pane shows "<name> · reconnecting…" placeholder. |
| 3.10 | Revoked by the host | `fletch-host devices revoke <id>` or the host desktop's revoke | Dot red, "This device is not paired with the host any more", **no retry**. Forget → switches to local first; switcher disappears when the last host is gone. |
| 3.11 | Saved hosts on relaunch | Relaunch with a saved host; also edit `remote.hosts` to an undialable `addr` | Dial starts after first paint; undialable → error row "Saved address … cannot be dialled. Pair this host again." |
| 3.12 | Local agents while remote is active | Start a local agent turn; switch to the host; let it finish; switch back | Status and transcript are correct on return (listeners were detached; `refreshWorkspace` catches up). Desktop notifications for the local agent during that window: note what happens. |
| 3.13 | Agent-name collision | Hard to force (names from a recycled pool). Proxy test: same agent selected on both envs, switch back and forth while one is mid-turn | No cross-contamination of `managedLogs`/`managedBusy`. **Known:** `autopilot` enrolment map and local DB prefs are keyed by bare agent id and not stashed (`environmentSwitch.ts:18-35`). |

### 3.14 Unknown-op paths (P1) — expected to degrade, must not crash or spin

These desktop calls are env-routed but not on the host allowlist, and are not
gated. Do each with the remote env active and note the behaviour; anything that
leaves a spinner, a stuck state, or a wrong-host side effect is a bug.

- **Attachments**: paste an image, drag a file, use the picker, then send.
  `save_pasted_attachment` writes on **this Mac** and the path is sent to the
  host (`files.ts:35-38`). The host cannot read it. Likely bug; confirm and
  decide.
- **Composer autocomplete**: `#` (PRs, issues) and `@` file mentions in a draft
  (`list_prs`, `list_tracker_issues`, `list_repo_tree`).
- **File panel**: edit/save, create, rename, delete, copy (`write_checkout_file`
  and friends). Read and diff should work.
- **Stats/usage screen** (`scan_usage_transcripts`).
- **Project screen**: delete project, attach/detach/relocate repo, rename,
  set label (`store/repos.ts:64-116`).
- **Account › Disconnect GitHub** while a remote env is active
  (`github_disconnect` is env-routed, `store/account.ts:92`). Should
  disconnect this Mac, will instead fail. Bug.
- **Slash commands** discovery and `run_claude_command`.
- **Sidebar PR badge sweep** (`refresh_all_pr_status`) — check console noise.
- **Live records for Cursor/Codex agents** (`append_live_record` fires per
  event): watch for an error flood in the console.
- **Agent already in native view on the host** opened from the client:
  expect a blank terminal, not a hang (`agent:output` is never forwarded).
- **Model picker**: shows this Mac's catalog for a remote agent (known quirk).

## 4. P1 — `fletch-host` headless on this Mac (checklist line 3c)

Build and run beside the desktop; the debug build takes its own
`~/Library/Application Support/fletch-host/dev` and `dev/{workspaces,rpc}`,
so nothing collides with the desktop.

```sh
cargo build --manifest-path crates/fletch-host/Cargo.toml
H=crates/fletch-host/target/debug/fletch-host
$H serve --port 47286 --name "mini"          # desktop may hold 47285
```

Debug on purpose: `data_dir_under` only appends `dev` under `debug_assertions`
(`crates/fletch-host/src/serve.rs:48-55`), so a release binary would serve from
`~/Library/Application Support/fletch-host` with flat `workspaces`/`rpc` and the
`dev` paths below would not exist.

| # | Check | Expect |
|---|---|---|
| 4.1 | `$H status` | JSON with `enabled, listening, port, name: "mini", hostId, addresses, devices, relay, dataDir, pid, version, agents{total,running}, sandboxEngine: "sandbox-exec", githubConnected`. |
| 4.2 | Second `serve` on the same data dir | "another fletch-host is already serving <dir>", exit 1. Kill -9 the first, start again: stale socket removed silently. |
| 4.3 | Data dir perms | `stat -f %Lp <dataDir>` = 700, `host.sock` = 600. `chmod 755` it and restart: back to 700. |
| 4.4 | `$H pair` | URL, QR, 8-char code, expiry. Phone (and the desktop client, §3) pair against it. |
| 4.5 | Agent sandbox on the host | From a phone-spawned agent: `cat <dataDir>/data.db`, `cat <dataDir>/remote/host_key`, `ls <dataDir>/remote` → denied; its own checkout under `<dataDir>/dev/workspaces/<agent>` and `<dataDir>/dev/rpc/<agent>` → allowed; sibling agent's rpc dir → denied; `<dataDir>/git-dist` read-only if present. |
| 4.6 | `project add` / `project clone --into` | Project appears on the phone/client immediately (`workspace:changed`). Relative path is absolutized against your cwd. |
| 4.7 | Approvals | `publish_confirmation` set on the host DB; `approvals list`, `approve <id>`, `approve <id> --deny`; let one expire. Answering an unknown id still prints "approved" (accepted quirk). |
| 4.8 | `devices list|revoke` | Revoke hangs up the device immediately (4003). |
| 4.9 | `github login` | Without `QUORUM_GITHUB_CLIENT_ID`: clear "not configured" error. With it: code + URL, waits, "signed in as …"; `status.githubConnected: true`; a push from a host agent works; token is in the settings table (plaintext, by design). After restart, `github_login_status` reads `idle` even though the token is stored. |
| 4.10 | Signals | `kill -TERM` mid-turn → agents killed, socket removed, exit 0; `serve` again → workflow runs resume, pending messages rehydrate. |
| 4.11 | `--no-relay`, `--relay wss://…`, bad `--relay` | Bad URL fails boot with a clear message; nothing from the CLI is persisted to the DB. |
| 4.12 | Not running | Any subcommand → "fletch-host is not running; start it with `fletch-host serve`", exit 1. |
| 4.13 | `service install` (launchd) | Writes `~/Library/LaunchAgents/com.fletch.host.plist`, bootstraps, prints status/log commands; `launchctl print gui/$(id -u)/com.fletch.host` shows it running; log at `~/Library/Logs/fletch-host.log`. Run install again with `--port` changed: rewritten and restarted. `--system` refused on macOS. `service uninstall` boots it out and removes the plist; running uninstall twice says "no agent at …". |
| 4.14 | `update --check` | Prints current/available/target/asset. Needs a release that ships host assets with `.sha256` and `.sig` (first one after v0.7.32). |
| 4.15 | `update` | Under `sudo` → refused. As yourself with the binary in `~/.local/bin`: downloads to `<dataDir>/updates/<v>/`, "checksum ok", "signature ok", DB copied to `<dataDir>/backups/`, atomic swap, restarts the launchd agent if the plist's ProgramArguments names this binary; with a bare `serve` running it says so and does **not** kill it. Binary in a root-owned dir → stops after verification and prints the `sudo … update --from` line; that line re-verifies the signature and refuses a dir that is not root's alone. Tamper a byte in the tarball → refused, nothing installed. |

## 5. P2 — Linux host (checklist line 3e; needs an Ubuntu box)

| # | Check | Expect |
|---|---|---|
| 5.1 | No Docker/Podman | `serve` refuses with the "no container runtime is available" message, exit 1. |
| 5.2 | Rootful Docker | Agent runs with `--user uid:gid` + tmpfs home; files in the checkout are owned by the service user; `apt-get` inside the container fails (documented). `FLETCH_DOCKER_TESTS=1` runs `docker_run_as_host_user_writes_user_owned_files`. |
| 5.3 | Rootless Docker / Podman | No `--user`; files still owned by you. |
| 5.4 | Login shell | `$SHELL` is respected for PATH discovery (`bin_resolve`); `claude` installed via Linuxbrew is found. |
| 5.5 | `service install` (systemd user) | Unit at `~/.config/systemd/user/fletch-host.service`; prints `loginctl enable-linger <user>`; survives SSH logout only after linger. `--system --user NAME` under sudo writes `/etc/systemd/system/…` with `User=` and the user's data dir; `--system` without a user or with root → refused. |
| 5.6 | Full acceptance | Phone pairs over LAN and relay, spawns, approves a push, opens a PR; `kill -TERM` mid-run then restart resumes. |
| 5.7 | Cursor on Linux | Known unusable in a mapped container; confirm the failure is legible. |

## 6. Things the audit flagged as likely bugs (verify, then file)

1. Attachments from a desktop client are sent as local paths to a remote host (§3.14).
2. "Disconnect GitHub" is routed to the active environment, so it fails while a remote host is active (§3.14).
3. Publish approval no longer fails closed instantly when the webview emit fails; it waits out the timeout (§1.7). Intentional per the sink docs, but changes desktop behaviour.
4. A phone that connects after a publish approval was requested never sees it (§2.4).
5. Pairing a second host on the phone silently replaces the first (§2.12).
6. `autopilot` enrolment and per-agent local DB prefs are keyed by bare agent id across environments (§3.13).
7. A symlinked `~/Library/Application Support` would emit the host deny block into the desktop seatbelt profile (§1.12).
