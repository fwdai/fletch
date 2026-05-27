# Remove Stopped State Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the `Stopped` status from the active-agent lifecycle — agents are always `Idle` when not running, sending a message to a dead-idle agent auto-resumes it, and Stop interrupts the current turn without killing the process.

**Architecture:** `stop_agent` sends SIGINT (managed) / `\x03` (PTY) to interrupt the running turn without exiting the process; on natural process exit `apply_exit_if_current` transitions to `Idle` (not `Stopped`); when `send_user_message` gets `AgentNotFound` the frontend auto-calls `resumeAgent` and retries.

**Tech Stack:** Rust (Tauri 2, `nix` 0.29 crate for SIGINT), TypeScript/React, Zustand

---

## File Map

| File | Change |
|------|--------|
| `src-tauri/Cargo.toml` | Add `nix` dep |
| `src-tauri/src/managed_session.rs` | Add `interrupt()` — sends SIGINT to child |
| `src-tauri/src/pty_session.rs` | Add `interrupt()` — writes `\x03` to PTY |
| `src-tauri/src/agent.rs` | Add `interrupt()` dispatch |
| `src-tauri/src/supervisor.rs` | `stop_agent` → interrupt only; `apply_exit_if_current` → `Idle` on clean exit |
| `src/store.ts` | `sendUserMessage` → auto-resume on `agent not found`; revert WorkspaceHeader changes |
| `src/components/Workspace/WorkspaceHeader.tsx` | Remove Resume button (revert) |
| `src/components/Workspace/ChatView.tsx` | Revert placeholder to "Agent is not ready" fallback |

---

### Task 1: Add `nix` crate and `ManagedSession::interrupt()`

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/managed_session.rs`

- [ ] **Step 1: Add `nix` to Cargo.toml**

In `src-tauri/Cargo.toml`, add after the `parking_lot` line:

```toml
nix = { version = "0.29", features = ["signal"] }
```

- [ ] **Step 2: Add `interrupt()` to `ManagedSession`**

In `src-tauri/src/managed_session.rs`, add this method after `send_user_message` (before `kill`):

```rust
    /// Send SIGINT to the child process to interrupt the current turn
    /// without killing it. Claude in stream-json mode handles SIGINT by
    /// aborting the current turn and emitting a result event, then
    /// returning to idle. If the process does not survive SIGINT the
    /// exit handler will transition the agent to Idle automatically.
    pub fn interrupt(&self) {
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;
            if let Some(child) = self.child.lock().as_ref() {
                if let Some(id) = child.id() {
                    let _ = kill(Pid::from_raw(id as i32), Signal::SIGINT);
                }
            }
        }
    }
```

- [ ] **Step 3: Verify it compiles**

```bash
cd src-tauri && cargo check 2>&1 | tail -20
```

Expected: no errors (warnings OK).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/managed_session.rs
git commit -m "feat(backend): add ManagedSession::interrupt() via SIGINT"
```

---

### Task 2: Add `PtySession::interrupt()` and `Agent::interrupt()`

**Files:**
- Modify: `src-tauri/src/pty_session.rs`
- Modify: `src-tauri/src/agent.rs`

- [ ] **Step 1: Add `interrupt()` to `PtySession`**

In `src-tauri/src/pty_session.rs`, add this method after `write` (before `resize`):

```rust
    /// Write Ctrl+C to the PTY to interrupt the currently running command
    /// without exiting the shell/process.
    pub fn interrupt(&self) -> Result<()> {
        self.write(&[0x03])
    }
```

- [ ] **Step 2: Add `interrupt()` to `Agent`**

In `src-tauri/src/agent.rs`, add this method after `send_user_message` (before `resize`):

```rust
    /// Interrupt the agent's current turn without terminating the process.
    /// For PTY agents this writes Ctrl+C; for managed agents this sends SIGINT.
    pub fn interrupt(&self) {
        match self {
            Self::Pty(a) => {
                let _ = a.pty.interrupt();
            }
            Self::Managed(a) => {
                a.session.interrupt();
            }
        }
    }
```

- [ ] **Step 3: Verify it compiles**

```bash
cd src-tauri && cargo check 2>&1 | tail -20
```

Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/pty_session.rs src-tauri/src/agent.rs
git commit -m "feat(backend): add Agent::interrupt() dispatching SIGINT/Ctrl-C by agent type"
```

---

### Task 3: Rewrite `stop_agent` — interrupt only, no kill, emit `Idle`

**Files:**
- Modify: `src-tauri/src/supervisor.rs`

The current `stop_agent` (lines 470–486) kills the process, removes it from all maps, and emits `Stopped`. Replace it entirely with an interrupt-only version.

- [ ] **Step 1: Replace `stop_agent`**

Find and replace the entire `stop_agent` function:

```rust
    pub async fn stop_agent(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
        // Interrupt the current turn without exiting the process.
        // The natural result-event + turn-watchdog path will transition
        // the agent to Idle once the interrupt is processed. If the
        // process does exit (e.g. it doesn't survive SIGINT), the exit
        // handler in apply_exit_if_current will also move it to Idle.
        let agents = self.agents.lock();
        if let Some(agent) = agents.get(agent_id) {
            agent.interrupt();
        }
        Ok(())
    }
```

- [ ] **Step 2: Verify it compiles**

```bash
cd src-tauri && cargo check 2>&1 | tail -20
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/supervisor.rs
git commit -m "feat(backend): stop_agent sends interrupt instead of killing process"
```

---

### Task 4: Clean process exit → `Idle` (not `Stopped`)

**Files:**
- Modify: `src-tauri/src/supervisor.rs`

The `apply_exit_if_current` function (around line 1199) currently sets `Stopped` on a successful exit. Change it to `Idle`.

- [ ] **Step 1: Change `Stopped` → `Idle` in `apply_exit_if_current`**

Find this block (around line 1199):

```rust
    let (status, err) = if success {
        (AgentStatus::Stopped, None)
    } else {
        (AgentStatus::Error, Some(format!("Agent process exited: {message}")))
    };
```

Replace with:

```rust
    let (status, err) = if success {
        // Clean exit means the agent is resumable — keep it Idle so the
        // user can send follow-up messages without a manual Resume step.
        (AgentStatus::Idle, None)
    } else {
        (AgentStatus::Error, Some(format!("Agent process exited: {message}")))
    };
```

- [ ] **Step 2: Verify it compiles**

```bash
cd src-tauri && cargo check 2>&1 | tail -20
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/supervisor.rs
git commit -m "feat(backend): clean process exit transitions to Idle instead of Stopped"
```

---

### Task 5: `sendUserMessage` — auto-resume dead-idle agents

**Files:**
- Modify: `src/store.ts`

When `send_user_message` is called for an agent with no live process, the backend returns `"agent not found: <id>"`. Detect this in the store action, call `resumeAgent`, then retry with `sendWhenAgentReady`.

- [ ] **Step 1: Update `sendUserMessage` in `store.ts`**

Find the `sendUserMessage` action (around line 531) and replace it:

```typescript
  sendUserMessage: async (id, text) => {
    try {
      set((state) => ({
        managedLogs: {
          ...state.managedLogs,
          [id]: [
            ...(state.managedLogs[id] ?? []),
            { kind: "user_message", text },
          ],
        },
        managedBusy: { ...state.managedBusy, [id]: true },
      }));
      try {
        await api.sendUserMessage(id, text);
      } catch (e) {
        if (String(e).includes("agent not found")) {
          // Dead idle agent (finished its prior task) — resume the
          // process in --resume mode, then deliver the message once ready.
          await api.resumeAgent(id);
          await sendWhenAgentReady(() => api.sendUserMessage(id, text));
        } else {
          throw e;
        }
      }
    } catch (e) {
      set((state) => ({
        lastError: String(e),
        managedBusy: { ...state.managedBusy, [id]: false },
      }));
    }
  },
```

- [ ] **Step 2: Run TypeScript check**

```bash
npm run check 2>&1
```

Expected: no errors (exits with no output).

- [ ] **Step 3: Commit**

```bash
git add src/store.ts
git commit -m "feat(frontend): auto-resume dead-idle agent on sendUserMessage"
```

---

### Task 6: Revert UI changes — remove Resume button, restore placeholder

**Files:**
- Modify: `src/components/Workspace/WorkspaceHeader.tsx`
- Modify: `src/components/Workspace/ChatView.tsx`

These were added in a prior attempt at a workaround. They're no longer needed.

- [ ] **Step 1: Remove Resume button from `WorkspaceHeader.tsx`**

Remove the `Chip` import line added earlier:

```typescript
import { Chip } from "../ui/Chip";
```

Remove the `resume` store subscription:

```typescript
  const resume = useAppStore((s) => s.resume);
```

Remove the Resume chip JSX block:

```tsx
      {(agent.status === "stopped" || agent.status === "error") && (
        <Chip tip="Resume this agent" onClick={() => resume(agent.id)}>
          <Icon name="play" size={11} />
          <span>Resume</span>
        </Chip>
      )}

```

- [ ] **Step 2: Restore original placeholder in `ChatView.tsx`**

Find the placeholder prop and revert to:

```tsx
          placeholder={
            canSend
              ? "Send a follow-up — ⌘↵ to send"
              : transcriptLoading
                ? "Loading transcript…"
                : switchInFlight
                  ? "Switching view…"
                : busy
                  ? "Waiting…"
                  : "Agent is not ready"
          }
```

- [ ] **Step 3: Run TypeScript check**

```bash
npm run check 2>&1
```

Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add src/components/Workspace/WorkspaceHeader.tsx src/components/Workspace/ChatView.tsx
git commit -m "refactor: remove Resume button (no longer needed — agents are always idle)"
```

---

### Task 7: Manual smoke test

- [ ] **Step 1: Build and run**

```bash
npm run tauri dev
```

- [ ] **Scenario A: Natural completion**
  1. Spawn an agent, give it a simple task ("write hello world to stdout")
  2. Watch it complete — status dot should stay grey/idle, NOT disappear or show stopped
  3. Composer should remain enabled with "Send a follow-up" placeholder
  4. Type a follow-up message and press ⌘↵
  5. Expected: agent auto-resumes (status briefly shows spawning/running), processes the message, returns to idle

- [ ] **Scenario B: Stop mid-turn**
  1. Spawn an agent, give it a long-running task
  2. While running, click the Stop button (red square) in the sidebar
  3. Expected: current turn interrupted, status returns to idle, composer re-enables
  4. Type a follow-up — agent should respond normally

- [ ] **Scenario C: App restart**
  1. Spawn an agent, let it complete a task (agent goes idle)
  2. Quit and relaunch the app
  3. Expected: agent is auto-resumed on startup (status shows spawning → idle)
  4. Composer is enabled, follow-up message works

- [ ] **Step 2: Commit if no regressions**

```bash
git add -A
git commit -m "test: manual smoke tests passed for no-stopped-state behavior"
```

---

## Notes

- `AgentStatus::Stopped` is kept in the Rust enum and frontend type — it is still used by `archive_agent` when snapshotting agents to history. Archived agents (shown in the History view) may have `stopped` status; this is correct and expected.
- The `archivable` guard in `AgentRow.tsx` (`idle | stopped | error`) keeps `stopped` as a guard condition — harmless since active agents will never carry that status, but leaving it preserves correct behavior for any edge cases.
- Error agents remain a terminal state requiring manual Resume via `api.resumeAgent`. This is intentional — an error indicates something went wrong that the user should investigate.
