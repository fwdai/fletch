// The Roadmap tab's project-manager chats: listing them, starting one, and
// retiring one.
//
// A "chat" here is a real workspace — its own clone, session and transcript —
// spawned with `purpose = ROADMAP_PM_PURPOSE`. That tag is what keeps it out of
// the sidebar (which is for feature-development agents and workflow runs) and
// what denies it the publish ops backend-side, so the PM can read every line of
// the codebase it reasons about and still never ship code.
//
// Several chats per project is the point, not an accident: one ever-growing
// thread ends up with a broken cache and a maxed-out context window, and a new
// chat costs kilobytes (`git clone --shared`). So "New chat" is a first-class
// action, and the picker keeps the old ones around until the user drops them.

import { useCallback, useEffect, useRef, useState } from "react";
import { type AgentRecord, api, onAgentStatus, onAgentTask, ROADMAP_PM_PURPOSE } from "@/api";
import { resolveAgentSpawnProfile, resolveBaseBranch } from "@/helpers";
import { PROJECT_MANAGER_NAME, PROJECT_MANAGER_PRESET } from "@/starterPack";
import { listCustomAgents } from "@/storage/customAgents";
import { useAppStore } from "@/store";

/** How the user picked the agent for a new chat: a custom agent (the Project
 *  Manager preset, or any other), or a bare provider. Mirrors what the
 *  composer's agent picker emits. */
export interface ChatAgentPick {
  provider: string;
  model?: string;
  customAgentId?: string;
}

/** In-flight seeding of the Project Manager preset, shared across mounts so two
 *  Roadmap tabs opening at once can't create it twice. */
let seeding: Promise<string | undefined> | null = null;

/** The Project Manager preset's id, creating it if this install has never had
 *  it. The starter pack seeds it too, but only when explicitly installed —
 *  which a Roadmap user may never have done — and the tab needs its default
 *  agent to actually exist. Idempotent by name, exactly like the installer, so
 *  the two paths can't produce duplicates. */
async function ensureProjectManager(): Promise<string | undefined> {
  if (!seeding) {
    seeding = (async () => {
      // Checked against the table, not the store's list: the list is loaded at
      // startup, and a check that raced that load would seed a duplicate the
      // user then has to clean up.
      const stored = await listCustomAgents();
      const existing = stored.find((a) => a.name === PROJECT_MANAGER_NAME);
      if (existing) return existing.id;
      const created = await useAppStore.getState().createCustomAgent(PROJECT_MANAGER_PRESET);
      return created.id;
    })()
      .catch(() => undefined)
      .finally(() => {
        seeding = null;
      });
  }
  return seeding;
}

export interface PmChatsState {
  chats: AgentRecord[];
  /** The chat the thread is rendering; null when the project has none yet. */
  selected: AgentRecord | null;
  select: (id: string) => void;
  loading: boolean;
  /** A spawn is in flight — the picker's start button waits on it. */
  starting: boolean;
  error: string | null;
  clearError: () => void;
  /** Spawn a new chat and open it. The workspace provisions in the background,
   *  by design: the chat is addressable the moment the record exists, so the
   *  user can start typing while the clone lands. */
  startChat: (pick: ChatAgentPick) => Promise<void>;
  /** Discard a chat for good — its record, checkout and transcript. */
  deleteChat: (id: string) => Promise<void>;
  /** The Project Manager preset's id, once resolved: the default pick for a new
   *  chat. Undefined only while it's being seeded, or if seeding failed. */
  defaultAgentId: string | undefined;
}

export function usePmChats(projectId: string | null, repoPath: string): PmChatsState {
  const registerOffSidebarAgents = useAppStore((s) => s.registerOffSidebarAgents);
  const discard = useAppStore((s) => s.discard);
  // Re-resolves whenever the library changes, so a Project Manager created in
  // Settings (or seeded below) becomes the default without a remount.
  const pmAgentId = useAppStore(
    (s) => s.customAgents.find((a) => a.name === PROJECT_MANAGER_NAME)?.id,
  );

  const [chats, setChats] = useState<AgentRecord[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Publish the records the workspace snapshot omits, so the store's by-id
  // lookups (provider → transcript adapter, repo → slash commands) resolve for
  // them. Mirrored from the list rather than written at each mutation site, so
  // there is one place where "what this surface owns" is stated.
  useEffect(() => {
    registerOffSidebarAgents(chats);
  }, [chats, registerOffSidebarAgents]);

  // The tab's default agent must exist before the picker can preselect it.
  useEffect(() => {
    void ensureProjectManager();
  }, []);

  // ── the list ───────────────────────────────────────────────────────
  useEffect(() => {
    if (!projectId) {
      setChats([]);
      setSelectedId(null);
      setLoading(false);
      return;
    }
    let alive = true;
    setLoading(true);
    api
      .listProjectChats(projectId, ROADMAP_PM_PURPOSE)
      .then((rows) => {
        if (!alive) return;
        setChats(rows);
        // Open on the newest conversation — where the user left off — rather
        // than on the first one they ever started.
        setSelectedId((current) =>
          current && rows.some((r) => r.id === current) ? current : (rows[0]?.id ?? null),
        );
        setLoading(false);
      })
      .catch((e) => {
        if (!alive) return;
        setError(String(e));
        setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [projectId]);

  // These records don't ride the workspace snapshot, so the two fields the
  // thread renders live — the running/idle state and the first message, which
  // becomes the chat's title — are patched from their own events.
  const patch = useCallback((id: string, fields: Partial<AgentRecord>) => {
    setChats((prev) =>
      prev.some((c) => c.id === id)
        ? prev.map((c) => (c.id === id ? { ...c, ...fields } : c))
        : prev,
    );
  }, []);

  useEffect(() => {
    const offStatus = onAgentStatus((e) =>
      patch(e.agent_id, { status: e.status, last_error: e.last_error }),
    );
    const offTask = onAgentTask((e) => patch(e.agent_id, { task: e.task }));
    return () => {
      void offStatus.then((f) => f());
      void offTask.then((f) => f());
    };
  }, [patch]);

  // ── actions ────────────────────────────────────────────────────────
  // Read through a ref so `startChat` doesn't change identity per keystroke of
  // the picker; it only ever needs the value at call time.
  const repoRef = useRef(repoPath);
  repoRef.current = repoPath;

  const startChat = useCallback(async (pick: ChatAgentPick) => {
    setStarting(true);
    setError(null);
    try {
      const repo = repoRef.current;
      // The preset's own reasoning budget: the PM is a "think hard" agent, and
      // the picker doesn't offer effort (there is no per-chat reason to lower
      // it), so it comes from the agent definition.
      const custom = useAppStore.getState().customAgents.find((a) => a.id === pick.customAgentId);
      const profile = resolveAgentSpawnProfile(
        useAppStore.getState(),
        pick.customAgentId,
        pick.provider,
      );
      const base = await resolveBaseBranch(repo);
      const rec = await api.spawnAgent(
        "custom",
        repo,
        pick.provider,
        // No pinned name: this chat was never a sidebar draft, so the backend
        // allocates one under the same lock that writes the row.
        undefined,
        custom?.effort ?? undefined,
        pick.model,
        profile.instructions,
        profile.customAgentId,
        base,
        profile.skills,
        profile.mcpServers,
        // Not issue work, and not a sidebar agent: the purpose tag is what
        // keeps it on this tab and off the publish path.
        undefined,
        ROADMAP_PM_PURPOSE,
      );
      setChats((prev) => [rec, ...prev]);
      setSelectedId(rec.id);
    } catch (e) {
      setError(String(e));
    } finally {
      setStarting(false);
    }
  }, []);

  const deleteChat = useCallback(
    async (id: string) => {
      // `discard` is the destructive one — record, checkout and transcript. A
      // PM chat has no branch and no PR, so there is nothing an archive would
      // preserve that the user would ever come back for.
      await discard(id);
      setChats((prev) => prev.filter((c) => c.id !== id));
      // Fall back to whatever is newest among the survivors, so deleting the
      // open chat lands the user in another conversation rather than nowhere.
      setSelectedId((current) =>
        current === id ? (chats.find((c) => c.id !== id)?.id ?? null) : current,
      );
    },
    [chats, discard],
  );

  const selected = chats.find((c) => c.id === selectedId) ?? null;

  return {
    chats,
    selected,
    select: setSelectedId,
    loading,
    starting,
    error,
    clearError: useCallback(() => setError(null), []),
    startChat,
    deleteChat,
    defaultAgentId: pmAgentId,
  };
}
