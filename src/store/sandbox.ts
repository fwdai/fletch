import { api, type DockerProbe } from "@/api";
import { DEFAULT_SANDBOX_ENGINE } from "@/storage/preferences";
import type { SandboxSlice, SliceCreator } from "./types";

export const createSandboxSlice: SliceCreator<SandboxSlice> = (set, get) => ({
  sandboxEngine: DEFAULT_SANDBOX_ENGINE,
  dockerProbe: null,

  setSandboxEngine: async (engine) => {
    const prev = get().sandboxEngine;
    // Optimistic: the backend validates docker against a live daemon probe
    // and can refuse (probe raced a daemon shutdown) — revert so the UI never
    // shows an engine that didn't persist.
    set({ sandboxEngine: engine });
    try {
      // The backend command persists the `sandbox_engine` setting AND updates
      // its in-memory spawn-path mirror, so we don't also call setSetting here
      // (same posture as `setTelemetryEnabled`).
      await api.setSandboxEngine(engine);
    } catch (e) {
      set({ sandboxEngine: prev, lastError: String(e) });
    }
  },
  refreshDockerProbe: async () => {
    let probe: DockerProbe;
    try {
      probe = await api.probeDockerEngine();
    } catch {
      // A failed probe means we can't confirm docker — treat as not installed
      // so the option gates off rather than dangling enabled.
      probe = { status: "not-installed" };
    }
    set({ dockerProbe: probe });
    return probe;
  },
});
