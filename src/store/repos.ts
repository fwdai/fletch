import { api } from "../api";
import type { SliceCreator, ReposSlice } from "./types";

export const createReposSlice: SliceCreator<ReposSlice> = (set) => ({
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

  createRepo: async (name, destParent, isPrivate, description) => {
    const ws = await api.createRepo(name, destParent, isPrivate, description);
    set({ workspace: ws });
  },
});
