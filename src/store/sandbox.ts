import { api, type ContainerAuthStatus, type DockerProbe } from "@/api";
import { DEFAULT_SANDBOX_ENGINE, type SandboxEngine } from "@/storage/preferences";
import type { SliceCreator } from "./types";

/** Live state of the embedded docker image build (first docker spawn). `null`
 *  when no build is in progress. `building` streams the latest output line;
 *  `failed` stays up (with `error`) until dismissed. Success clears to `null`. */
export interface DockerBuildProgress {
  status: "building" | "failed";
  /** Most recent `docker build` output line (building only). */
  lastLine: string | null;
  /** Failure reason (failed only). */
  error: string | null;
}

export interface SandboxSlice {
  /** Engine new agents are stamped with. Mirrors the backend-owned
   *  `sandbox_engine` setting; existing agents keep their stamped engine. */
  sandboxEngine: SandboxEngine;
  /** Latest Docker availability probe; `null` until the first probe lands. */
  dockerProbe: DockerProbe | null;
  /** Which container auth chain step is active (Anthropic credentials for
   *  docker agents); `null` until the first refresh lands. */
  containerAuth: ContainerAuthStatus | null;
  /** Live image-build progress for the build toast; `null` = no build. */
  dockerBuild: DockerBuildProgress | null;
  /** Advanced docker launch knobs, mirrored from the backend-owned
   *  `docker_image` / `docker_memory` / `docker_cpus` settings. Empty string =
   *  unset (the launch path uses its defaults: 4g memory, 2 cpus, embedded
   *  image). */
  dockerImage: string;
  dockerMemory: string;
  dockerCpus: string;

  /** Persist a new engine choice via the backend, which validates docker
   *  against a live daemon probe — reverts the store on refusal. */
  setSandboxEngine: (engine: SandboxEngine) => Promise<void>;
  /** Re-probe Docker availability into `dockerProbe` (settings pane open).
   *  Returns the probe result so callers can poll until the daemon answers. */
  refreshDockerProbe: () => Promise<DockerProbe>;
  /** Re-resolve the container auth chain into `containerAuth`. */
  refreshContainerAuth: () => Promise<void>;
  /** Persist a pasted `claude setup-token` for containers, then refresh the
   *  status. Throws on backend refusal (empty token) so the connect modal
   *  can show the error inline. */
  setContainerAuthToken: (token: string) => Promise<void>;
  /** Drop the stored container token, then refresh the status (a later chain
   *  step — shell env / credentials file — may take over). */
  clearContainerAuthToken: () => Promise<void>;
  /** Dismiss the build toast (used after a failed build). */
  dismissDockerBuild: () => void;
  /** Persist the advanced docker launch knobs (image / memory / cpus) via the
   *  backend, which writes the settings AND updates the spawn-path mirror.
   *  Reverts the store on failure. */
  saveDockerLaunchSettings: (image: string, memory: string, cpus: string) => Promise<void>;
  /** Launch Docker Desktop (daemon-down error action); surfaces failures via
   *  `lastError`. */
  startDockerDesktop: () => Promise<void>;
}

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
