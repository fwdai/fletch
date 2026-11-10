import { api, type Workspace } from "@/api";
import { normalizeFocus } from "@/components/Sidebar/projectFocus";
import { dropAgentEntries } from "@/helpers";
import { clearOutputBuffer } from "@/pty/buffers";
import { remapProjectOrder } from "@/storage/projectOrder";
import { projectKey } from "@/util/projects";
import { track } from "@/util/track";
import { activeGateReason, type GateName } from "./capabilities";
import { focusedProjectCount, projectKeys } from "./projectFocus";
import type { AppState, SliceCreator } from "./types";

/** Refuse a write the environment the user is driving cannot take. The control
 *  is already disabled with this reason, so this is the backstop for the paths
 *  that never touch a button — Enter in the name field, a blur that saves a
 *  label — as the Git panel's dispatch is for its own keyboard shortcuts.
 *  Throws, because every caller here surfaces the failure inline. */
function refuseWhenGated(gate: GateName = "projectAdmin"): void {
  const reason = activeGateReason(gate);
  if (reason) throw new Error(reason);
}

/** The primary repo path of the project `after` has and `before` did not —
 *  what an add just pinned, or `null` when it pinned nothing. A new project
 *  has one repo, so the first unseen ref is it. */
function addedProjectPath(before: Workspace | null | undefined, after: Workspace): string | null {
  const known = new Set(before?.projects.map((p) => p.path) ?? []);
  return after.projects.find((p) => !known.has(p.path))?.path ?? null;
}

/** A project added while the sidebar is narrowed to a focus joins that focus.
 *  The list shows only what is picked, so a project left out of it would be
 *  added and nowhere to be seen. With no focus on there is nothing to join.
 *
 *  Which projects there are — and so whether the saved keys are a focus at
 *  all, and whether the result names every project — takes the workflow runs
 *  (see `store/projectFocus`), which the store doesn't hold. They are asked
 *  for only when something is saved; the usual add never waits on them.
 *
 *  The answer can be slow, and it is only good for the question asked: if the
 *  focus was changed or the environment switched meanwhile, the user has
 *  moved on and the join is dropped rather than applied to a newer choice. */
async function joinProjectFocus(get: () => AppState, path: string): Promise<void> {
  const asked = get();
  if (asked.projectFocus.length === 0) return;
  const runs = await api.wfListRuns().catch(() => []);
  const s = get();
  if (s.projectFocus !== asked.projectFocus) return;
  if (s.activeEnvironmentId !== asked.activeEnvironmentId) return;
  const ref = s.workspace?.projects.find((p) => p.path === path);
  if (!ref || focusedProjectCount(s, runs) === 0) return;
  s.setProjectFocus(normalizeFocus([...s.projectFocus, projectKey(ref)], projectKeys(s, runs)));
}

export interface ReposSlice {
  /** Pin an existing local repo. Every successful add hands off to a draft on
   *  the new project (`draftAfterProjectAdd`); `draft: false` skips that, for
   *  a caller whose screen the user should stay on. */
  addWorkspaceRepo: (path: string, opts?: { draft?: boolean }) => Promise<void>;
  removeWorkspaceRepo: (path: string) => Promise<void>;
  // Attach/detach resolve on success and throw on failure, so the Project
  // Settings Repositories section can show the error inline.
  /** Attach a repo to an existing project (multi-repo projects). */
  attachRepoToProject: (projectId: string, path: string) => Promise<void>;
  /** Detach a repo from a project (guarded backend-side). */
  detachRepoFromProject: (projectId: string, path: string) => Promise<void>;
  /** Set a repo's display label; blank clears to the basename fallback. */
  setRepoLabel: (path: string, label: string) => Promise<void>;
  // Rename/relocate resolve on success and throw on failure, so the Project
  // Settings modal can show the error inline rather than in the global banner.
  /** Set a project's custom display name (independent of its folder). */
  renameProject: (projectId: string, name: string) => Promise<void>;
  /** Choose the project's icon (`api.projectIconSet`); throws for inline display. */
  setProjectIcon: (projectId: string, source: string | null, bytes?: string) => Promise<void>;
  /** Delete a project and all agents/workspaces belonging to it. */
  deleteProject: (projectId: string) => Promise<void>;
  /** Repoint a pinned repo at a moved folder, migrating its sidebar order and
   *  the open settings modal to the new path. */
  relocateProject: (oldPath: string, newPath: string) => Promise<void>;
  /** Open the log folder in the OS file manager; surfaces failures via
   *  `lastError` rather than swallowing them. */
  revealLogs: () => Promise<void>;
  // Clone/create resolve on success and throw on failure, so the New Project
  // modal can show the error inline rather than in the global banner.
  cloneRepo: (spec: string, destParent: string) => Promise<void>;
  createRepo: (
    name: string,
    destParent: string,
    isPrivate: boolean,
    description?: string,
    /** Also create + push to GitHub. False = local-only (no connection yet). */
    publish?: boolean,
  ) => Promise<void>;
}

export const createReposSlice: SliceCreator<ReposSlice> = (set, get) => ({
  addWorkspaceRepo: async (path, opts) => {
    set({ busy: true, lastError: null });
    try {
      const before = get().workspace;
      const first = (before?.projects.length ?? 0) === 0;
      const ws = await api.addWorkspaceRepo(path);
      set({ workspace: ws });
      // A path that is already pinned is a successful no-op on the backend:
      // nothing was added, so nothing is counted and nothing opens.
      const added = addedProjectPath(before, ws);
      if (!added) return;
      // Activation: onboarding hands off to an empty sidebar, so the first
      // project is the moment a new user actually starts. Method + first only —
      // never the path.
      track("project_added", { method: "existing", first });
      void joinProjectFocus(get, added);
      if (opts?.draft !== false) void get().draftAfterProjectAdd(added, first);
    } catch (e) {
      set({ lastError: String(e) });
    } finally {
      set({ busy: false });
    }
  },

  removeWorkspaceRepo: async (path) => {
    // The one action here that reports through `lastError` rather than
    // throwing; the refusal follows it.
    const gated = activeGateReason("projectAdmin");
    if (gated) {
      set({ lastError: gated });
      return;
    }
    try {
      const ws = await api.removeWorkspaceRepo(path);
      set({ workspace: ws });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  attachRepoToProject: async (projectId, path) => {
    // Errors propagate to the Repositories section for inline display.
    refuseWhenGated();
    const ws = await api.attachRepoToProject(projectId, path);
    set({ workspace: ws });
  },

  detachRepoFromProject: async (projectId, path) => {
    refuseWhenGated();
    const ws = await api.detachRepoFromProject(projectId, path);
    set({ workspace: ws });
  },

  setRepoLabel: async (path, label) => {
    refuseWhenGated();
    const ws = await api.setRepoLabel(path, label);
    set({ workspace: ws });
  },

  renameProject: async (projectId, name) => {
    // Errors propagate to the modal for inline display.
    refuseWhenGated();
    const ws = await api.renameProject(projectId, name);
    set({ workspace: ws });
  },

  setProjectIcon: async (projectId, source, bytes) => {
    refuseWhenGated("projectIcon");
    const ws = await api.projectIconSet(projectId, source, bytes);
    set({ workspace: ws });
  },

  deleteProject: async (projectId) => {
    refuseWhenGated();
    const result = await api.deleteProject(projectId);
    const deletedIds = result.deleted_agent_ids;
    for (const id of deletedIds) clearOutputBuffer(id);
    set((state) => {
      let patch = {};
      for (const id of deletedIds)
        patch = { ...patch, ...dropAgentEntries({ ...state, ...patch }, id) };
      return {
        ...patch,
        workspace: result.workspace,
        selectedAgentId: deletedIds.includes(state.selectedAgentId ?? "")
          ? null
          : state.selectedAgentId,
        selectedRunId: result.deleted_run_ids.includes(state.selectedRunId ?? "")
          ? null
          : state.selectedRunId,
        projectScreen: null,
      };
    });
  },

  relocateProject: async (oldPath, newPath) => {
    refuseWhenGated();
    const ws = await api.relocateRepo(oldPath, newPath);
    // Keep the project's manual sidebar position, and repoint the open project
    // view (keyed by repo path) at the new location so it doesn't go stale.
    remapProjectOrder(oldPath, newPath);
    // Drafts are keyed by repo path; carry any in-progress composer over to the
    // new location so it follows the move instead of being dropped from the view.
    set((state) => ({
      workspace: ws,
      drafts: state.drafts.map((d) => (d.repoPath === oldPath ? { ...d, repoPath: newPath } : d)),
      projectScreen:
        state.projectScreen?.repoPath === oldPath
          ? { ...state.projectScreen, repoPath: newPath }
          : state.projectScreen,
    }));
  },

  revealLogs: async () => {
    try {
      await api.revealLogs();
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  cloneRepo: async (spec, destParent) => {
    // The new project appears in the sidebar via the refreshed workspace.
    // Errors propagate to the caller (the modal) for inline display — and skip
    // the event, so `project_added` only ever means a project that landed.
    const before = get().workspace;
    const first = (before?.projects.length ?? 0) === 0;
    const ws = await api.cloneRepo(spec, destParent);
    set({ workspace: ws });
    track("project_added", { method: "clone", first });
    const added = addedProjectPath(before, ws);
    if (!added) return;
    void joinProjectFocus(get, added);
    void get().draftAfterProjectAdd(added, first);
  },

  createRepo: async (name, destParent, isPrivate, description, publish) => {
    // The popover's Create row is already dead on a host that cannot take it;
    // this is the same backstop the settings writes have, for a modal left open
    // across an environment switch.
    refuseWhenGated("createProject");
    const before = get().workspace;
    const first = (before?.projects.length ?? 0) === 0;
    const ws = await api.createRepo(name, destParent, isPrivate, description, publish);
    set({ workspace: ws });
    track("project_added", { method: "create", first });
    const added = addedProjectPath(before, ws);
    if (!added) return;
    void joinProjectFocus(get, added);
    void get().draftAfterProjectAdd(added, first);
  },
});
