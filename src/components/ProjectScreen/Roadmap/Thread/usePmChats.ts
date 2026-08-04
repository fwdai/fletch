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
//
// This hook also owns the *standup digest*: the board keeps moving with the tab
// closed (runs dispatch, PRs open, the sweep ships), so opening an existing chat
// that is behind the board asks it, once, to say what happened. The rule for
// "behind" is a pure function in `standup.ts`; everything here is the plumbing
// and the once-per-session guards.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  type AgentRecord,
  api,
  onAgentStatus,
  onAgentTask,
  ROADMAP_PM_PURPOSE,
  type UserTurn,
} from "@/api";
import { resolveAgentSpawnProfile, resolveBaseBranch } from "@/helpers";
import { PROJECT_MANAGER_NAME, PROJECT_MANAGER_PRESET } from "@/starterPack";
import { listCustomAgents } from "@/storage/customAgents";
import { useAppStore } from "@/store";
import { chatActiveAt, STANDUP_PROMPT, shouldAskForStandup } from "./standup";

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

// ── the standup digest ───────────────────────────────────────────────
// Module-level rather than per-mount, for the same reason `seeding` is: the
// Roadmap tab remounts on every navigation, and a guard that remounted with it
// would ask again every time the user came back to the tab.

/** Projects that have had their one digest this app session. */
const askedProjects = new Set<string>();

/** Chats spawned in this app session. Their opening turn *is* the conversation,
 *  so there is nothing for a digest to summarize — and the spawn may have
 *  dispatched a first message that hasn't persisted a turn row yet, which would
 *  otherwise read as "no turns, therefore stale". */
const freshChats = new Set<string>();

/** In-flight digest checks, by project. The check awaits two fetches before it
 *  decides, and `selected` re-identifies on every status event — so without this
 *  two overlapping runs could both pass the `askedProjects` guard and send two
 *  digests. Mirrors `seeding`'s shape. */
const checkingStandup = new Map<string, Promise<void>>();

/** Ask the opened chat for a digest, if the board moved since it was last live.
 *  Never throws: a failed digest is a message the user didn't get, not an error
 *  worth putting on the Roadmap tab. */
async function askForStandup(projectId: string, chat: AgentRecord): Promise<void> {
  if (askedProjects.has(projectId)) return;
  const inFlight = checkingStandup.get(projectId);
  if (inFlight) return inFlight;
  const run = (async () => {
    const [event, turns] = await Promise.all([
      api.roadmapLatestEvent(projectId).catch(() => null),
      api.readUserTurns(chat.id).catch(() => [] as UserTurn[]),
    ]);
    const ask = shouldAskForStandup({
      boardMovedAt: event?.created_at ?? null,
      chatActiveAt: chatActiveAt(chat, turns),
      freshlySpawned: freshChats.has(chat.id),
      // Re-read after the awaits: another project's check can't have set it, but
      // a remount racing this one could.
      alreadyAsked: askedProjects.has(projectId),
    });
    if (!ask) return;
    // Claimed before the send, so a failure doesn't retry on the next remount —
    // the digest is a nicety, and one that keeps re-firing is worse than one
    // that was missed.
    askedProjects.add(projectId);
    // The digest lands as an ordinary user turn in this chat, which is what makes
    // it self-limiting across app restarts too: the next session reads it as the
    // chat's last activity.
    await useAppStore.getState().sendUserMessage(chat.id, STANDUP_PROMPT);
  })()
    .catch(() => {})
    .finally(() => {
      checkingStandup.delete(projectId);
    });
  checkingStandup.set(projectId, run);
  return run;
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
  /** Spawn a new chat and open it, sending `firstMessage` as its opening turn
   *  when the new-chat screen collected one. The workspace provisions in the
   *  background, by design: the chat is addressable the moment the record
   *  exists, so the user can start typing while the clone lands — and for the
   *  same reason the opening turn can be dispatched right away, with the
   *  backend holding it until the process is up.
   *
   *  Resolves `true` once the chat exists and is open. A `false` means the spawn
   *  failed and `error` says why — the new-chat screen stays up on that, rather
   *  than dropping the user back into the conversation they left. */
  startChat: (pick: ChatAgentPick, firstMessage?: string) => Promise<boolean>;
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
  // them. This mirror keeps the registry fresh as rows are patched, but it is
  // NOT the load-bearing registration: effects flush child-first, so on the
  // commit where `chats` first arrives, ChatPane's transcript load would
  // resolve its provider before this parent effect ran and mis-adapt a
  // non-claude chat's history. The fetch `.then` and `startChat` therefore
  // register synchronously, before the record can render.
  useEffect(() => {
    registerOffSidebarAgents(chats);
  }, [chats, registerOffSidebarAgents]);

  // Report the on-screen chat so turn-end signals know the user is watching it
  // (an off-sidebar chat can never be `selectedAgentId` — see `attendedChatId`).
  const setAttendedChat = useAppStore((s) => s.setAttendedChat);
  useEffect(() => {
    setAttendedChat(selectedId);
    return () => setAttendedChat(null);
  }, [selectedId, setAttendedChat]);

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
        // Register before rendering: ChatPane resolves its transcript adapter
        // synchronously on mount, ahead of this hook's mirror effect.
        useAppStore.getState().registerOffSidebarAgents(rows);
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

  // ── the standup digest ─────────────────────────────────────────────
  // Read the opened chat through a ref: the records re-identify on every status
  // event (see `patch`), and depending on the object would re-run this on each
  // one. The id is the only thing that means "a different conversation opened".
  const chatsRef = useRef(chats);
  chatsRef.current = chats;
  useEffect(() => {
    // `loading` gates it so the digest is decided against the real list, not the
    // empty one this hook starts with.
    if (!projectId || loading || !selectedId) return;
    const chat = chatsRef.current.find((c) => c.id === selectedId);
    if (chat) void askForStandup(projectId, chat);
  }, [projectId, loading, selectedId]);

  // ── actions ────────────────────────────────────────────────────────
  // Read through a ref so `startChat` doesn't change identity per keystroke of
  // the picker; it only ever needs the value at call time.
  const repoRef = useRef(repoPath);
  repoRef.current = repoPath;

  const startChat = useCallback(async (pick: ChatAgentPick, firstMessage?: string) => {
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
      // Register before rendering — same reason as the list load above.
      useAppStore.getState().registerOffSidebarAgents([rec]);
      setChats((prev) => [rec, ...prev]);
      setSelectedId(rec.id);
      // The opening turn, dispatched without waiting: the send appends its
      // optimistic bubble to this agent's log, which is what the pane mounting
      // on the next render reads, so the message is already on screen when the
      // transcript appears. Failures land in the store's own error channel —
      // this hook's `error` is about the spawn, and the chat now exists.
      // Never digest into a chat this session created: there is no "since we
      // last spoke", and the opening turn below may not have persisted a row yet.
      freshChats.add(rec.id);
      const opening = firstMessage?.trim();
      if (opening) void useAppStore.getState().sendUserMessage(rec.id, opening);
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    } finally {
      setStarting(false);
    }
  }, []);

  const deleteChat = useCallback(
    async (id: string) => {
      // Drop the chat locally FIRST, so its pane unmounts before the backend
      // teardown: `discard` prunes the dead agent's transcript keys mid-flight,
      // and a still-mounted ChatPane would refetch history for an agent that no
      // longer exists and re-orphan those keys.
      // Fall back to whatever is newest among the survivors, so deleting the
      // open chat lands the user in another conversation rather than nowhere.
      const survivors = chats.filter((c) => c.id !== id);
      setChats(survivors);
      setSelectedId((current) => (current === id ? (survivors[0]?.id ?? null) : current));
      try {
        // `discard` is the destructive one — record, checkout and transcript. A
        // PM chat has no branch and no PR, so there is nothing an archive would
        // preserve that the user would ever come back for.
        await discard(id);
      } catch (e) {
        // The optimistic removal was wrong — put the truth back on screen.
        setError(String(e));
        if (projectId) {
          const rows = await api.listProjectChats(projectId, ROADMAP_PM_PURPOSE).catch(() => null);
          if (rows) {
            useAppStore.getState().registerOffSidebarAgents(rows);
            setChats(rows);
          }
        }
      }
    },
    [chats, discard, projectId],
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
