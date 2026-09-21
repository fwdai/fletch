import { api } from "@/api";
import { dropAgentEntries } from "@/helpers";
import { clearOutputBuffer } from "@/pty/buffers";
import { remapProjectOrder } from "@/storage/projectOrder";
import { track } from "@/util/track";
import { activeGateReason, type GateName } from "./capabilities";
import type { SliceCreator } from "./types";

/** Refuse a write the environment the user is driving cannot take. The control
 *  is already disabled with this reason, so this is the backstop for the paths
 *  that never touch a button — Enter in the name field, a blur that saves a
 *  label — as the Git panel's dispatch is for its own keyboard shortcuts.
 *  Throws, because every caller here surfaces the failure inline. */
function refuseWhenGated(gate: GateName = "projectAdmin"): void {
  const reason = activeGateReason(gate);
  if (reason) throw new Error(reason);
}

export interface ReposSlice {
  addWorkspaceRepo: (path: string) => Promise<void>;
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
  addWorkspaceRepo: async (path) => {
    set({ busy: true, lastError: null });
    try {
      const first = (get().workspace?.projects.length ?? 0) === 0;
      const ws = await api.addWorkspaceRepo(path);
      set({ workspace: ws });
      // Activation: onboarding hands off to an empty sidebar, so the first
      // project is the moment a new user actually starts. Method + first only —
      // never the path.
      track("project_added", { method: "existing", first });
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
        projectScreenRepoPath: null,
      };
    });
  },

  relocateProject: async (oldPath, newPath) => {
    refuseWhenGated();
    const ws = await api.relocateRepo(oldPath, newPath);
    // Keep the project's manual sidebar position, and repoint the open project
    // screen (keyed by repo path) at the new location so it doesn't go stale.
    remapProjectOrder(oldPath, newPath);
    // Drafts are keyed by repo path; carry any in-progress composer over to the
    // new location so it follows the move instead of being dropped from the view.
    set((state) => ({
      workspace: ws,
      drafts: state.drafts.map((d) => (d.repoPath === oldPath ? { ...d, repoPath: newPath } : d)),
    }));
    if (get().projectScreenRepoPath === oldPath) get().openProjectScreen(newPath);
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
    const first = (get().workspace?.projects.length ?? 0) === 0;
    const ws = await api.cloneRepo(spec, destParent);
    set({ workspace: ws });
    track("project_added", { method: "clone", first });
  },

  createRepo: async (name, destParent, isPrivate, description, publish) => {
    // The popover's Create row is already dead on a host that cannot take it;
    // this is the same backstop the settings writes have, for a modal left open
    // across an environment switch.
    refuseWhenGated("createProject");
    const first = (get().workspace?.projects.length ?? 0) === 0;
    const ws = await api.createRepo(name, destParent, isPrivate, description, publish);
    set({ workspace: ws });
    track("project_added", { method: "create", first });
  },
});
