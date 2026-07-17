import { api } from "@/api";
import { dropAgentEntries } from "@/helpers";
import { clearOutputBuffer } from "@/pty/buffers";
import { remapProjectOrder } from "@/storage/projectOrder";
import type { ReposSlice, SliceCreator } from "./types";

export const createReposSlice: SliceCreator<ReposSlice> = (set, get) => ({
  addWorkspaceRepo: async (path) => {
    set({ busy: true, lastError: null });
    try {
      const ws = await api.addWorkspaceRepo(path);
      set({ workspace: ws });
    } catch (e) {
      set({ lastError: String(e) });
    } finally {
      set({ busy: false });
    }
  },

  removeWorkspaceRepo: async (path) => {
    try {
      const ws = await api.removeWorkspaceRepo(path);
      set({ workspace: ws });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  attachRepoToProject: async (projectId, path) => {
    // Errors propagate to the Repositories section for inline display.
    const ws = await api.attachRepoToProject(projectId, path);
    set({ workspace: ws });
  },

  detachRepoFromProject: async (projectId, path) => {
    const ws = await api.detachRepoFromProject(projectId, path);
    set({ workspace: ws });
  },

  setRepoLabel: async (path, label) => {
    const ws = await api.setRepoLabel(path, label);
    set({ workspace: ws });
  },

  renameProject: async (projectId, name) => {
    // Errors propagate to the modal for inline display.
    const ws = await api.renameProject(projectId, name);
    set({ workspace: ws });
  },

  deleteProject: async (projectId) => {
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
        projectSettingsRepoPath: null,
      };
    });
  },

  relocateProject: async (oldPath, newPath) => {
    const ws = await api.relocateRepo(oldPath, newPath);
    // Keep the project's manual sidebar position, and repoint the open settings
    // modal (keyed by repo path) at the new location so it doesn't go stale.
    remapProjectOrder(oldPath, newPath);
    set({ workspace: ws });
    if (get().projectSettingsRepoPath === oldPath) get().openProjectSettings(newPath);
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
    // Errors propagate to the caller (the modal) for inline display.
    const ws = await api.cloneRepo(spec, destParent);
    set({ workspace: ws });
  },

  createRepo: async (name, destParent, isPrivate, description, publish) => {
    const ws = await api.createRepo(name, destParent, isPrivate, description, publish);
    set({ workspace: ws });
  },
});
