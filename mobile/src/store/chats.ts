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
import type { MobileState } from "./index";

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
  /** Apply a live event to a chat record. A no-op for an agent that is not in
   *  the registry, so the event handlers can call it unconditionally. */
  patchChat(agentId: string, fields: Partial<AgentRecord>): void;
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

/** The spawn profile a planning chat runs under: the Mac's own Project Manager
 *  row when its library has one — so an edited brief is honoured and the chat is
 *  attributed to that agent — and the bundled preset otherwise.
 *
 *  The phone never seeds the library: creating a custom agent is the desktop's
 *  call, and a spawn with the preset's instructions inline behaves the same. */
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
        set((s) => ({
          chats: { ...s.chats, [projectId]: [record, ...(s.chats[projectId] ?? [])] },
        }));
        await firstTurn({
          record,
          prompt,
          attachments,
          recover: () => get().loadChats(projectId),
        });
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
