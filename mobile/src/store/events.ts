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
import type { RoadmapItem } from "@desktop/api/types/roadmap";
import type { PublishApproval, PublishApprovalResolved } from "@desktop/api/types/sandbox";
import type {
  SessionRecordsAppendedEvent,
  TurnSentEvent,
  TurnStartedEvent,
} from "@desktop/api/types/session";
import { mirrorSentTurn } from "@desktop/helpers/mirrorTurn";
import type { RawEvent } from "../adapters";
import { ignore } from "../lib/ignore";
import type { RemoteClient } from "../remote";
import { agentOf, type MobileState } from "./index";
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
    const provider = agentOf(get(), e.agent_id)?.provider;
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
    // A planning chat's record is not in the snapshot, so it is patched in its
    // own registry — this is what lets `waitForSpawn` and the status line work
    // for one. A no-op for a sidebar agent.
    get().patchChat(e.agent_id, { status: e.status, last_error: e.last_error ?? null });
    set((s) => {
      const ws = s.workspace;
      const turnStartedAt = { ...s.turnStartedAt };
      if (e.status === "idle" || e.status === "error" || e.status === "stopped") {
        delete turnStartedAt[e.agent_id];
      }
      return {
        workspace: ws
          ? patchAgent(ws, e.agent_id, {
              status: e.status,
              last_error: e.last_error ?? undefined,
            })
          : ws,
        busy:
          e.status === "running"
            ? { ...s.busy, [e.agent_id]: true }
            : { ...s.busy, [e.agent_id]: false },
        turnStartedAt,
      };
    });
  });

  // A user message the host accepted, from whichever device sent it. Mirror it
  // so a prompt typed on the Mac shows here while its turn is still running;
  // our own send is already in the log under its turnId (see `send`) and is
  // skipped. A turn-opening message asserts busy the way `send` does.
  on<TurnSentEvent>("turn:sent", (e) => {
    set((s) => {
      const prev = s.logs[e.agent_id] ?? [];
      const next = mirrorSentTurn(prev, e);
      if (next === prev) return {};
      return {
        logs: { ...s.logs, [e.agent_id]: next },
        busy: e.follow_up ? s.busy : { ...s.busy, [e.agent_id]: true },
      };
    });
  });

  on<TurnStartedEvent>("turn:started", (e) => {
    set((s) => ({ turnStartedAt: { ...s.turnStartedAt, [e.agent_id]: e.started_at } }));
  });

  on<AgentTaskEvent>("agent:task", (e) => {
    // The task is a chat's title, so it is patched in both registries too.
    get().patchChat(e.agent_id, { task: e.task });
    const ws = get().workspace;
    if (ws) set({ workspace: patchAgent(ws, e.agent_id, { task: e.task }) });
  });

  on<AgentModelEvent>("agent:model", (e) => {
    // The composer's model picker reads the selection off the record, so a chat
    // has to be patched in its own registry too — the snapshot never holds it.
    get().patchChat(e.agent_id, { model: e.model });
    const ws = get().workspace;
    if (ws) set({ workspace: patchAgent(ws, e.agent_id, { model: e.model }) });
  });

  on<AgentEffortEvent>("agent:effort", (e) => {
    get().patchChat(e.agent_id, { effort: e.effort });
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
    // A commit or discard empties the working tree, which is what the rows
    // show — don't make them wait out the poll interval to catch up.
    void get().loadShortstats();
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

  // A roadmap row that was written, wherever it was written from. The phone
  // only draws one kind — the PM's `proposed` ghosts, as decision cards in the
  // planning chat that raised them — and which rows those are is the slice's
  // call, so the whole row goes to it and a ruled-on one drops out there.
  on<RoadmapItem>("roadmap:item", (item) => {
    get().applyRoadmapItem(item);
  });

  // The payload is the bare item id: a ghost discarded here or on the Mac.
  on<string>("roadmap:item-deleted", (id) => {
    get().removeRoadmapItem(id);
  });

  // A gated publish the host is blocked on. Control plane, not transcript: it
  // goes to its own list, which the agent's chat draws as an Approve / Deny
  // card. Answering needs `answer_publish_approval`, which a host older than
  // the op does not have — the card handles that, not this tap.
  on<PublishApproval>("publish:approval-requested", (request) => {
    get().receivePublishApproval(request);
  });

  // …and its closing half: the request is over, whoever ended it — this phone,
  // the Mac, a host terminal, or nobody before the wait lapsed. The card goes,
  // and nothing is sent back. A host that never emits this leaves the card up
  // until it is answered, which is what every client did before the event.
  on<PublishApprovalResolved>("publish:approval-resolved", (e) => {
    get().resolvePublishApproval(e.id);
  });

  // `verify:report` is forwarded by the host but has no v1 surface on the phone
  // (no verification card), so it is intentionally unhandled.
}
