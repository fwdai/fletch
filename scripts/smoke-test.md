# Manual smoke test

End-to-end verification that `algiers` actually spawns a working agent. Run
this on a clean machine the first time you set up, and after any changes to
the spawn/teardown flow.

## Prereqs

- macOS 13+ on Apple Silicon
- Repo cloned, `npm install` has run (downloaded Tart into
  `src-tauri/resources/tart/`)
- Base image `base-dev` has been built per
  [`build-base-image.md`](build-base-image.md)
- An `ANTHROPIC_API_KEY` is set inside the base image, or `claude` has been
  authenticated inside it (otherwise `claude` will block on auth and the
  smoke test stalls)

## 1. App boots

```bash
npm run tauri dev
```

Expected:
- A window titled "algiers" opens.
- Header shows "algiers" and "No repo selected".
- Sidebar has "+ Spawn" disabled.
- No errors in the dev console (toggle with Cmd-Option-I).

If the app errors at startup with "bundled tart binary not found", re-run
`bash scripts/download-tart.sh`.

## 2. Workspace selection persists

- Click "Choose repo…" → pick any local git repo.
- When prompted for the Tart base image, enter `base-dev`.
- The header updates to show the repo basename + base image badge.
- Close the app, reopen. The same workspace should load automatically (state
  is in `~/Library/Application Support/com.algiers.app/workspaces.json`).

## 3. Single-agent spawn works end-to-end

- Click "+ Spawn".
- Name: `smoke-1`
- Branch: defaults to `agent/smoke-1` on blur — accept it.
- Task: `Print "hello from inside the VM" and then exit. Do nothing else.`
- Click Spawn.

Expected sequence (watch terminal pane + sidebar status dot):
1. Status `spawning` (yellow dot). Sidebar shows the new agent.
2. Within ~5–10s: status `running` (green dot), xterm pane attaches.
3. `claude` prompt appears in the pane, processes the task, prints "hello…",
   exits.
4. Worktree is visible on the host at `<repo>/.worktrees/<id>/`.

## 4. Two agents in parallel

- Spawn `smoke-2` with task `Wait 30 seconds, then print "two done".`
- Immediately spawn `smoke-3` with task `Print "three done" and exit.`
- Switch between them via the sidebar — each terminal stays attached.
- Verify `tart list` from a separate shell shows both `algiers-<id>` VMs.
- Verify each agent has its own worktree under `.worktrees/`.

## 5. Stop + cleanup

- Click Stop on smoke-1.
- Confirm.
- Sidebar status flips to `stopped`. Click "Discard worktree" to remove the
  worktree.
- Verify in another shell:
  - `tart list` no longer shows that agent's VM
  - `.worktrees/<id>` is gone
  - The branch still exists (`git branch | grep agent/`); intentional —
    user may want to merge.

## 6. Failure mode: virtiofs not mounted

If the base image is missing the virtiofs kernel module or the mount step
fails, the spawn will reach `running` status briefly then fail. Surface
should be: agent moves to `error`, last_error shows `mount: unknown
filesystem type 'virtiofs'` or similar. The VM is left in place for
inspection until you click "Discard worktree".

## What this test does NOT cover (yet)

- Long-running interactive sessions (more than ~10 minutes) — not exercised
  here; rely on production usage to find issues.
- Concurrent spawns >4 — memory pressure on the host gets real.
- App-restart-while-agents-running — agents reappear as `stopped` (their
  VMs may still be running); no reattach UX in v1.
