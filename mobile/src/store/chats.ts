// Planning chats: the project-manager conversations a project keeps beside its
// agents. They are ordinary agent records tagged with `ROADMAP_PM_PURPOSE`, and
// that tag is what keeps them out of the `get_workspace` snapshot — so they
// arrive through `list_project_chats` and live in their own registry here,
// keyed by project. Everything downstream (the log, the composer, the agent
// screen) addresses them by agent id like any other agent; `agentOf` is what
// bridges the two registries.

import { type AgentRecord, ROADMAP_PM_PURPOSE } from "@desktop/api/types/agent";
import { PROJECT_MANAGER_NAME, PROJECT_MANAGER_PRESET } from "@desktop/starterPack/presets";
import type { Api } from "../api";
import { agentOf, type MobileState } from "./index";

type Set = (partial: Partial<MobileState> | ((s: MobileState) => Partial<MobileState>)) => void;
type Get = () => MobileState;

export interface PlanningChatInput {
  projectId: string;
  /** The idea, dictated or typed — it becomes the chat's opening turn. */
  prompt: string;
  /** Staged host paths to go with it, if any. */
  attachments?: string[];
}

export interface ChatsSlice {
  /** Purpose-tagged chats per project id, newest first — what the host answers
   *  `list_project_chats` with, patched in place by live agent events. */
  chats: Record<string, AgentRecord[]>;
  loadChats(projectId: string): Promise<void>;
  /** Spawn a Project Manager chat for a project and open it on the idea. */
  startPlanningChat(input: PlanningChatInput): Promise<void>;
  /** Delete a planning chat outright — record, checkout and transcript, as the
   *  desktop's own `deleteChat` does. Archiving one would strand it: an archived
   *  chat is in neither the snapshot nor `list_project_chats`, so nothing on the
   *  phone could ever list it again. Needs `discard_agent`. */
  deleteChat(agentId: string): Promise<void>;
  /** Apply a live event to a chat record. A no-op for an agent that is not in
   *  the registry, so the event handlers can call it unconditionally. */
  patchChat(agentId: string, fields: Partial<AgentRecord>): void;
  /** Make an agent id resolvable before a screen mounts on it. A tapped
   *  notification (or a deep link) carries nothing but an id, and on a cold
   *  launch both registries are empty — so the record is fetched by id, which
   *  is the only read that reaches a chat the snapshot hides. Advisory: a
   *  failure leaves the screen as it was. */
  ensureAgent(agentId: string): Promise<void>;
}

/** What the slice borrows from the store it composes into, so it can be a
 *  module of its own without importing the store back. */
export interface ChatsDeps {
  api: Api;
  /** Record a failure where the error UI can see it, then rethrow. */
  guard<T>(fn: () => Promise<T>): Promise<T>;
  /** The optimistic-turn → wait-for-spawn → first-message path a spawn takes,
   *  shared with `spawn` so both open an agent the same way. */
  firstTurn(input: {
    record: AgentRecord;
    prompt: string;
    attachments: string[];
    recover: () => Promise<void>;
  }): Promise<void>;
}

/** Put a record in its project's list — replacing the one already there, or
 *  prepending it as the newest chat. Every other project's array keeps its
 *  reference, which is what the selectors reading them need. */
function upsert(chats: Record<string, AgentRecord[]>, record: AgentRecord) {
  const list = chats[record.project_id] ?? [];
  return {
    ...chats,
    [record.project_id]: list.some((c) => c.id === record.id)
      ? list.map((c) => (c.id === record.id ? record : c))
      : [record, ...list],
  };
}

/** The spawn profile a planning chat runs under. The phone only *names* it: it
 *  sends the Mac's Project Manager row id, and the host resolves that row by
 *  value — its brief, its skills and its MCP servers — so an edited brief is
 *  honoured, the chat is attributed to that agent, and no MCP configuration
 *  crosses the wire.
 *
 *  The inline `instructions` are the fallback and nothing else: the host uses
 *  them only for a library with no Project Manager row. The phone never seeds
 *  one — creating a custom agent is the desktop's call, and a spawn carrying
 *  the bundled preset's brief behaves the same. */
async function projectManager(api: Api) {
  const row = await api
    .listCustomAgents()
    .then((rows) => rows.find((a) => a.name === PROJECT_MANAGER_NAME))
    // An older host has no such op, and a library read is not worth failing a
    // spawn over: the preset below says everything the host needs.
    .catch(() => undefined);
  const preset = row ?? PROJECT_MANAGER_PRESET;
  return {
    provider: preset.base,
    model: preset.model,
    effort: preset.effort,
    instructions: preset.instructions,
    customAgentId: row?.id ?? null,
  };
}

export function createChatsSlice(set: Set, get: Get, deps: ChatsDeps): ChatsSlice {
  const { api, guard, firstTurn } = deps;
  return {
    chats: {},

    async loadChats(projectId) {
      // Nothing to ask an older host, and asking would answer "unknown op".
      if (!get().hostSupports("list_project_chats")) return;
      try {
        const rows = await api.listProjectChats(projectId, ROADMAP_PM_PURPOSE);
        set((s) => ({ chats: { ...s.chats, [projectId]: rows } }));
      } catch {
        // Advisory, like the other list reads: the section stays as it was and
        // the next mount or reconnect tries again.
      }
    },

    async startPlanningChat({ projectId, prompt, attachments = [] }) {
      return guard(async () => {
        const project = get().workspace?.projects.find((p) => p.project_id === projectId);
        if (!project) throw new Error("that project is not on the host");
        const pm = await projectManager(api);
        const [name, forkBase] = await Promise.all([
          // The chat was never a sidebar draft, but the spawn op still wants a
          // name, so one is drawn here exactly as `spawn` draws it.
          api.allocateDraftName([]),
          api.repoDefaultBranch(project.path).catch(() => "main"),
        ]);
        const record = await api.spawnAgent(
          project.path,
          pm.provider,
          name,
          pm.effort,
          pm.model,
          forkBase,
          {
            instructions: pm.instructions,
            customAgentId: pm.customAgentId,
            // The tag is what keeps this chat off the sidebar and off the
            // publish path — a PM reads the code, it never ships it.
            purpose: ROADMAP_PM_PURPOSE,
          },
        );
        // Registered before the screen opens: `agentOf` resolves from here, and
        // the agent screen mounts on the very next frame.
        set((s) => ({ chats: upsert(s.chats, record) }));
        await firstTurn({
          record,
          prompt,
          attachments,
          recover: () => get().loadChats(projectId),
        });
      });
    },

    async ensureAgent(agentId) {
      // Already resolvable — the snapshot has it, or this registry does.
      if (agentOf(get(), agentId)) return;
      if (!get().hostSupports("get_agent")) return;
      try {
        const record = await api.getAgent(agentId);
        if (!record) return;
        // A purpose tag is exactly what keeps a record out of the snapshot, so
        // it is this registry's to hold. An untagged record is a sidebar agent
        // that was spawned after the snapshot was read — the workspace is what
        // has to catch up there, not this list.
        if (record.purpose) set((s) => ({ chats: upsert(s.chats, record) }));
        else await get().refreshWorkspace();
      } catch {
        // Advisory: the screen stays as it was rather than the open failing,
        // and the next reconnect or open tries again.
      }
    },

    async deleteChat(agentId) {
      const chat = agentOf(get(), agentId);
      return guard(async () => {
        // Dropped from the registry FIRST, so the agent screen unmounts before
        // the host tears the transcript down — the desktop does the same: a
        // still-mounted chat would refetch the history of an agent that is
        // being deleted under it.
        if (chat) {
          set((s) => ({
            chats: {
              ...s.chats,
              [chat.project_id]: (s.chats[chat.project_id] ?? []).filter((c) => c.id !== agentId),
            },
          }));
        }
        get().closeSheet();
        const top = get()
          .nav.filter((i) => i.phase !== "leave")
          .at(-1);
        if (top?.screen === "agent" && top.props.agentId === agentId) get().pop();
        try {
          await api.discardAgent(agentId);
        } catch (e) {
          // The removal was wrong — put the host's truth back on screen.
          if (chat) await get().loadChats(chat.project_id);
          throw e;
        }
      });
    },

    patchChat(agentId, fields) {
      set((s) => {
        const projectId = Object.keys(s.chats).find((pid) =>
          s.chats[pid].some((c) => c.id === agentId),
        );
        if (!projectId) return {};
        return {
          chats: {
            ...s.chats,
            [projectId]: s.chats[projectId].map((c) =>
              c.id === agentId ? { ...c, ...fields } : c,
            ),
          },
        };
      });
    },
  };
}
