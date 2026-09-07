// Host events folded into store state, following the same shape as the
// desktop's src/store/eventListeners.ts for the v1 whitelist in
// docs/remote-protocol.md. Every event the doc forwards is either handled here
// or deliberately ignored (see the tail of `registerRemoteEvents`).

import type {
  AgentBranchEvent,
  AgentEffortEvent,
  AgentGitActionEvent,
  AgentManagedEvent,
  AgentModelEvent,
  AgentRepoAddedEvent,
  AgentStatusEvent,
  AgentTaskEvent,
  Workspace,
} from "@desktop/api/types/agent";
import type { PrStateChangedEvent } from "@desktop/api/types/pr";
import type { SessionRecordsAppendedEvent, TurnStartedEvent } from "@desktop/api/types/session";
import type { RawEvent } from "../adapters";
import { ignore } from "../lib/ignore";
import type { RemoteClient } from "../remote";
import type { MobileState } from "./index";
import { applyLiveEvent } from "./transcript";

type Set = (partial: Partial<MobileState> | ((s: MobileState) => Partial<MobileState>)) => void;
type Get = () => MobileState;

const patchAgent = (
  ws: Workspace,
  agentId: string,
  patch: Partial<Workspace["agents"][number]>,
): Workspace => ({
  ...ws,
  agents: ws.agents.map((a) => (a.id === agentId ? { ...a, ...patch } : a)),
});

/** A held `can_use_tool` prompt: control plane, not transcript. Record
 *  tool_use id → request id so the approval card can answer it, and never feed
 *  it to the reducer. */
function foldControlRequest(set: Set, agentId: string, ev: RawEvent): boolean {
  if (ev.type !== "control_request") return false;
  const request = ev.request as Record<string, unknown> | undefined;
  const requestId = ev.request_id;
  const toolUseId = request?.tool_use_id;
  if (request?.subtype === "can_use_tool" && typeof toolUseId === "string" && requestId) {
    set((s) => ({
      pendingToolUse: {
        ...s.pendingToolUse,
        [agentId]: { ...(s.pendingToolUse[agentId] ?? {}), [toolUseId]: String(requestId) },
      },
    }));
  }
  return true;
}

export function registerRemoteEvents(client: RemoteClient, set: Set, get: Get): void {
  const on = <T>(event: string, cb: (payload: T) => void) =>
    client.on(event, (payload) => cb(payload as T));

  on<AgentManagedEvent>("agent:event", (e) => {
    const raw = e.event as RawEvent;
    if (foldControlRequest(set, e.agent_id, raw)) return;
    const provider = get().workspace?.agents.find((a) => a.id === e.agent_id)?.provider;
    const { items, turnEnded } = applyLiveEvent(provider, get().logs[e.agent_id] ?? [], raw);
    set((s) => ({
      logs: { ...s.logs, [e.agent_id]: items },
      busy: turnEnded ? { ...s.busy, [e.agent_id]: false } : s.busy,
      pendingToolUse: turnEnded ? { ...s.pendingToolUse, [e.agent_id]: {} } : s.pendingToolUse,
    }));
  });

  // The canonical transcript for a finished turn — richer than the live render
  // (tool results the live stream dropped), so rebuild from it.
  on<SessionRecordsAppendedEvent>("session:records-appended", (e) => {
    void get().rebuildLog(e.agent_id).catch(ignore);
  });

  on<AgentStatusEvent>("agent:status", (e) => {
    const ws = get().workspace;
    if (!ws) return;
    set((s) => {
      const turnStartedAt = { ...s.turnStartedAt };
      if (e.status === "idle" || e.status === "error" || e.status === "stopped") {
        delete turnStartedAt[e.agent_id];
      }
      return {
        workspace: patchAgent(ws, e.agent_id, {
          status: e.status,
          last_error: e.last_error ?? undefined,
        }),
        busy:
          e.status === "running"
            ? { ...s.busy, [e.agent_id]: true }
            : { ...s.busy, [e.agent_id]: false },
        turnStartedAt,
      };
    });
  });

  on<TurnStartedEvent>("turn:started", (e) => {
    set((s) => ({ turnStartedAt: { ...s.turnStartedAt, [e.agent_id]: e.started_at } }));
  });

  on<AgentTaskEvent>("agent:task", (e) => {
    const ws = get().workspace;
    if (ws) set({ workspace: patchAgent(ws, e.agent_id, { task: e.task }) });
  });

  on<AgentModelEvent>("agent:model", (e) => {
    const ws = get().workspace;
    if (ws) set({ workspace: patchAgent(ws, e.agent_id, { model: e.model }) });
  });

  on<AgentEffortEvent>("agent:effort", (e) => {
    const ws = get().workspace;
    if (ws) set({ workspace: patchAgent(ws, e.agent_id, { effort: e.effort }) });
  });

  on<AgentBranchEvent>("agent:branch", (e) => {
    const ws = get().workspace;
    const agent = ws?.agents.find((a) => a.id === e.agent_id);
    if (!ws || !agent) return;
    set({
      workspace: patchAgent(ws, e.agent_id, {
        repos: agent.repos.map((r) => (r.subdir === e.subdir ? { ...r, branch: e.branch } : r)),
      }),
    });
  });

  on<AgentRepoAddedEvent>("agent:repo_added", (e) => {
    const ws = get().workspace;
    const agent = ws?.agents.find((a) => a.id === e.agent_id);
    if (!ws || !agent) return;
    set({ workspace: patchAgent(ws, e.agent_id, { repos: [...agent.repos, e.repo] }) });
  });

  // Ground truth that a git mutation landed: re-read the checkout's git and PR
  // state rather than guessing what changed.
  on<AgentGitActionEvent>("agent:git-action", (e) => {
    void get().loadGit(e.agent_id);
  });

  on<PrStateChangedEvent>("pr:state_changed", (e) => {
    set((s) => ({ prStates: { ...s.prStates, [e.agent_id]: e.state } }));
    if (e.state) void get().loadGit(e.agent_id);
  });

  // Archive/restore reshapes `repos` and `archive`, which agent:status doesn't
  // cover — reload the snapshot.
  on<null>("workspace:changed", () => {
    void get().refreshWorkspace();
  });

  // `verify:report` and `publish:approval-requested` are forwarded by the host
  // but have no v1 surface on the phone (no verification card, and publish
  // approval is a desktop-side setting), so they are intentionally unhandled.
}
