import { api, type DockerProbe } from "@/api";
import { DEFAULT_SANDBOX_ENGINE } from "@/storage/preferences";
import type { SandboxSlice, SliceCreator } from "./types";

export const createSandboxSlice: SliceCreator<SandboxSlice> = (set, get) => ({
  sandboxEngine: DEFAULT_SANDBOX_ENGINE,
  dockerProbe: null,
  dockerBuild: null,
  dockerImage: "",
  dockerMemory: "",
  dockerCpus: "",

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

  containerAuth: null,
  refreshContainerAuth: async () => {
    try {
      set({ containerAuth: await api.getContainerAuthStatus() });
    } catch {
      // A failed resolution means we can't confirm any credentials — show the
      // "Not connected" state (with its connect CTA) rather than a stale one.
      set({ containerAuth: { status: "none" } });
    }
  },
  setContainerAuthToken: async (token) => {
    // No try/catch: the connect modal surfaces the rejection (empty token)
    // inline next to the paste field, so the error must propagate.
    await api.setContainerAuthToken(token);
    await get().refreshContainerAuth();
  },
  clearContainerAuthToken: async () => {
    try {
      await api.clearContainerAuthToken();
    } catch (e) {
      set({ lastError: String(e) });
    }
    await get().refreshContainerAuth();
  },

  dismissDockerBuild: () => set({ dockerBuild: null }),

  saveDockerLaunchSettings: async (image, memory, cpus) => {
    const prev = {
      dockerImage: get().dockerImage,
      dockerMemory: get().dockerMemory,
      dockerCpus: get().dockerCpus,
    };
    // Optimistic, reverted on backend refusal — same posture as setSandboxEngine.
    set({ dockerImage: image, dockerMemory: memory, dockerCpus: cpus });
    try {
      await api.setDockerLaunchSettings(image || null, memory || null, cpus || null);
    } catch (e) {
      set({ ...prev, lastError: String(e) });
    }
  },

  startDockerDesktop: async () => {
    try {
      await api.startDockerDesktop();
    } catch (e) {
      set({ lastError: String(e) });
    }
  },
});
