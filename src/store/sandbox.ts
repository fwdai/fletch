import { api, type DockerProbe } from "@/api";
import { DEFAULT_SANDBOX_ENGINE } from "@/storage/preferences";
import type { SandboxSlice, SliceCreator } from "./types";

type StoreSet = Parameters<SliceCreator<SandboxSlice>>[0];

/** Apply an optimistic `patch`, reverting to `prev` (and surfacing the error)
 *  if the backend write rejects. These settings persist through a backend
 *  command that can refuse — e.g. the docker engine racing a daemon shutdown —
 *  so the UI must never keep a value that didn't stick. */
async function optimistic(
  set: StoreSet,
  patch: Partial<SandboxSlice>,
  prev: Partial<SandboxSlice>,
  persist: () => Promise<void>,
) {
  set(patch);
  try {
    await persist();
  } catch (e) {
    set({ ...prev, lastError: String(e) });
  }
}

export const createSandboxSlice: SliceCreator<SandboxSlice> = (set, get) => ({
  sandboxEngine: DEFAULT_SANDBOX_ENGINE,
  dockerProbe: null,
  dockerBuild: null,
  dockerImage: "",
  dockerMemory: "",
  dockerCpus: "",

  setSandboxEngine: (engine) =>
    // The backend command persists the `sandbox_engine` setting AND updates its
    // in-memory spawn-path mirror, so we don't also call setSetting here (same
    // posture as `setTelemetryEnabled`).
    optimistic(set, { sandboxEngine: engine }, { sandboxEngine: get().sandboxEngine }, () =>
      api.setSandboxEngine(engine),
    ),
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

  saveDockerLaunchSettings: (image, memory, cpus) =>
    optimistic(
      set,
      { dockerImage: image, dockerMemory: memory, dockerCpus: cpus },
      {
        dockerImage: get().dockerImage,
        dockerMemory: get().dockerMemory,
        dockerCpus: get().dockerCpus,
      },
      () => api.setDockerLaunchSettings(image || null, memory || null, cpus || null),
    ),

  startDockerDesktop: async () => {
    try {
      await api.startDockerDesktop();
    } catch (e) {
      set({ lastError: String(e) });
    }
  },
});
