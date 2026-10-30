import {
  api,
  type BoxEgressEvent,
  type BoxImageStatus,
  type BoxRuntimeProbe,
  type BoxSettingKey,
  type BoxStateEvent,
  type BoxToolsStatus,
  type ContainerAuthStatus,
  type DockerBuildEvent,
  type DockerProbe,
  type PodmanProbe,
  type PublishApproval,
  type PublishApprovalResolved,
} from "@/api";
import { hostSupports } from "@/remote/types";
import { DEFAULT_SANDBOX_ENGINE, type SandboxEngine } from "@/storage/preferences";
import { newestWins } from "@/util/newestWins";
import { type ApprovalEvent, replayApprovalEvents } from "@/util/publishApprovals";
import { activeEnvironment, forActiveEnvironment } from "./environments";
import type { SliceCreator } from "./types";

/** Live state of one embedded container image build (first spawn under a
 *  runtime). `building` streams the latest output line; `failed` stays up
 *  (with `error`) until dismissed; success removes the entry. The runtime is
 *  the `containerBuilds` key rather than a field here — Docker and Podman
 *  builds run on independent locks and can overlap. */
export interface DockerBuildProgress {
  status: "building" | "failed";
  /** Most recent `build` output line (building only). */
  lastLine: string | null;
  /** Failure reason (failed only). */
  error: string | null;
}

/** `containerBuilds` key for events that didn't name a runtime — the toast
 *  shows neutral copy for it instead of guessing. */
export const NEUTRAL_BUILD_RUNTIME = "container";

/** Every handshake starts an `approvals_list` without waiting for the last, so
 *  two can be out at once and answer out of order. One order for all of them,
 *  as everywhere else a whole list is replaced (util/newestWins). */
const approvalLoads = newestWins();

/** Approval events seen while an `approvals_list` is in flight, replayed over
 *  its answer so the snapshot cannot undo them (see util/publishApprovals).
 *  Belongs to the newest claim — an older load's snapshot is superseded
 *  wholesale, so the events it missed are not its to replay. Null when no list
 *  is outstanding, which is nearly always. */
let approvalsInFlight: ApprovalEvent[] | null = null;

/** Fold one `docker:build-progress` event into the keyed build state that
 *  drives the toast. Every phase touches only its own runtime's key, so
 *  overlapping builds don't clear each other. */
export function applyBuildEvent(
  builds: Record<string, DockerBuildProgress>,
  e: DockerBuildEvent,
): Record<string, DockerBuildProgress> {
  const key = e.runtime ?? NEUTRAL_BUILD_RUNTIME;
  switch (e.phase) {
    case "started":
      // Overwrites, so a retry clears the previous attempt's displayed error.
      return { ...builds, [key]: { status: "building", lastLine: null, error: null } };
    case "line": {
      // Materializes a missing entry: a reload mid-build loses the `started`,
      // and the toast would stay hidden for the rest of a minutes-long build.
      const build: DockerBuildProgress = builds[key] ?? {
        status: "building",
        lastLine: null,
        error: null,
      };
      return { ...builds, [key]: { ...build, lastLine: e.line ?? build.lastLine } };
    }
    case "finished": {
      if (!(key in builds)) return builds;
      const { [key]: _done, ...rest } = builds;
      return rest;
    }
    case "failed":
      return {
        ...builds,
        [key]: { status: "failed", lastLine: null, error: e.error ?? "Image build failed" },
      };
    default:
      return builds;
  }
}

export interface SandboxSlice {
  /** Engine new agents are stamped with. Mirrors the backend-owned
   *  `sandbox_engine` setting; existing agents keep their stamped engine. */
  sandboxEngine: SandboxEngine;
  /** Latest Docker availability probe; `null` until the first probe lands. */
  dockerProbe: DockerProbe | null;
  /** Latest Podman availability probe; `null` until the first probe lands. */
  podmanProbe: PodmanProbe | null;
  /** Which container auth chain step is active (Anthropic credentials for
   *  docker agents); `null` until the first refresh lands. */
  containerAuth: ContainerAuthStatus | null;
  /** Live image-build progress for the build toast, keyed by the runtime
   *  display name from the event stream ("Docker" / "Podman", or
   *  [`NEUTRAL_BUILD_RUNTIME`]) so one runtime's `finished` can't clear the
   *  other's still-running toast. Empty = no build in flight. */
  containerBuilds: Record<string, DockerBuildProgress>;
  /** Advanced docker launch knobs, mirrored from the backend-owned
   *  `docker_image` / `docker_memory` / `docker_cpus` settings. Empty string =
   *  unset (the launch path uses its defaults: 4g memory, 2 cpus, embedded
   *  image). */
  dockerImage: string;
  dockerMemory: string;
  dockerCpus: string;
  /** The same three knobs for podman, under separate settings keys so a user
   *  running both engines configures each. */
  podmanImage: string;
  podmanMemory: string;
  podmanCpus: string;
  /** Whether an agent must get the user's approval before publishing. Mirrors
   *  the backend-owned `publish_confirmation` setting. Off by default. */
  publishConfirmation: boolean;
  /** Publishes waiting on the user, oldest first. Two agents can be waiting at
   *  once, so this is a queue rather than a single slot; the prompt shows the
   *  head. Unanswered requests are denied backend-side after a timeout, so a
   *  dropped entry fails closed. */
  pendingPublishApprovals: PublishApproval[];
  /** The box sandbox's developer flag, mirrored from the host-owned
   *  `boxes_enabled`. Everything box-related on the Sandbox page renders only
   *  while this is on. */
  boxesEnabled: boolean;
  /** The box knobs, mirrored from the host-owned `box_*` settings. Empty
   *  string = unset (the engine's defaults apply). */
  boxMemoryMib: string;
  boxCpus: string;
  boxImageUrl: string;
  boxIdleStopMinutes: string;
  /** Latest box runtime probe; `null` until the first lands. */
  boxProbe: BoxRuntimeProbe | null;
  /** The pinned guest image's install state: `box_image_status` once, then
   *  every `box:image`. `null` until the first read lands. */
  boxImage: BoxImageStatus | null;
  /** Each supported provider's tools disk by provider id: seeded by the probe,
   *  then every `box:tools`. */
  boxTools: Record<string, BoxToolsStatus>;
  /** Each box agent's phase by agent id, from `box:state`. */
  boxStates: Record<string, BoxStateEvent>;
  /** Hosts a box's egress proxy refused, one entry per workspace and host
   *  however many connections it took, oldest first. */
  boxEgressBlocked: BlockedHost[];

  /** Persist a new engine choice via the backend, which validates docker
   *  against a live daemon probe — reverts the store on refusal. */
  setSandboxEngine: (engine: SandboxEngine) => Promise<void>;
  /** Turn the publish-approval prompt on or off (backend-owned setting). */
  setPublishConfirmation: (enabled: boolean) => Promise<void>;
  /** Queue a publish the backend is blocked on for the prompt. What the user
   *  already authorized — a delegation's or an autopilot-enrolled checkout's own
   *  pushes — the host approves without asking, so it never arrives here. */
  receivePublishApproval: (request: PublishApproval) => void;
  /** Replace the queue with what the active host says is still waiting.
   *
   *  `publish:approval-requested` only reaches the clients connected when it
   *  fired, so a window that switched to a host — or reconnected to one — has
   *  neither the prompts raised while it was away nor any way to know the ones
   *  it holds were answered elsewhere. Wholesale, because the host's registry
   *  is the authority on both halves of that.
   *
   *  A no-op for the local environment (this window was listening) and for a
   *  host too old for `approvals_list` (it would answer `unknown op`). */
  loadPendingPublishApprovals: () => Promise<void>;
  /** Answer the queued request `id` and drop it from the queue. */
  answerPublishApproval: (id: string, approved: boolean) => Promise<void>;
  /** Drop the queued request `id` because the engine says it is over — someone
   *  else answered it, or nobody did before the host's wait ran out. No call
   *  back to the engine: the verdict is already made. */
  resolvePublishApproval: (e: PublishApprovalResolved) => void;
  /** Re-probe Docker availability into `dockerProbe` (settings pane open).
   *  Returns the probe result so callers can poll until the daemon answers. */
  refreshDockerProbe: () => Promise<DockerProbe>;
  /** Re-probe Podman availability into `podmanProbe`. */
  refreshPodmanProbe: () => Promise<PodmanProbe>;
  /** Re-resolve the container auth chain into `containerAuth`. */
  refreshContainerAuth: () => Promise<void>;
  /** Persist a pasted `claude setup-token` for containers, then refresh the
   *  status. Throws on backend refusal (empty token) so the connect modal
   *  can show the error inline. */
  setContainerAuthToken: (token: string) => Promise<void>;
  /** Drop the stored container token, then refresh the status (a later chain
   *  step — shell env / credentials file — may take over). */
  clearContainerAuthToken: () => Promise<void>;
  /** Dismiss one runtime's build toast (used after a failed build). */
  dismissDockerBuild: (runtime: string) => void;
  /** Persist the advanced docker launch knobs (image / memory / cpus) via the
   *  backend, which writes the settings AND updates the spawn-path mirror.
   *  Reverts the store on failure. */
  saveDockerLaunchSettings: (image: string, memory: string, cpus: string) => Promise<void>;
  /** The podman twin of `saveDockerLaunchSettings`. */
  savePodmanLaunchSettings: (image: string, memory: string, cpus: string) => Promise<void>;
  /** Launch Docker Desktop (daemon-down error action); surfaces failures via
   *  `lastError`. */
  startDockerDesktop: () => Promise<void>;
  /** Flip the box developer flag (host-owned `boxes_enabled`, written through
   *  `set_box_setting`). On the host, turning it on is the whole setup: the
   *  image download, the tools disks, and `box` as the engine for new agents;
   *  those arrive back as events and `settings:changed`. Reverts the toggle
   *  on refusal (an Intel Mac, no runtime). */
  setBoxesEnabled: (enabled: boolean) => Promise<void>;
  /** Write one box knob; blank restores the default. Resolves to the host's
   *  validation message, or `null` when it stuck — the field shows it inline.
   *  The stored value arrives back as `settings:changed`, so no optimistic
   *  write here. */
  saveBoxSetting: (key: BoxSettingKey, value: string) => Promise<string | null>;
  /** Re-probe the box runtime into `boxProbe`; a host that cannot answer
   *  leaves it as is. */
  refreshBoxProbe: () => Promise<void>;
  /** Read the image's install state into `boxImage`. */
  refreshBoxImage: () => Promise<void>;
  /** Start (or retry) the image install and the tools builds after it, or
   *  cancel the download; refusals go to `lastError`. */
  installBoxImage: () => Promise<void>;
  cancelBoxImage: () => Promise<void>;
  /** Build a provider's tools disk now, past the backoff after failed
   *  builds; the outcome arrives as `box:tools`, refusals go to `lastError`. */
  retryBoxTools: (provider: string) => Promise<void>;
  applyBoxImage: (e: BoxImageStatus) => void;
  applyBoxState: (e: BoxStateEvent) => void;
  applyBoxTools: (e: BoxToolsStatus) => void;
  /** Fold a refused connection into its workspace's blocked hosts. A new
   *  host that is not known noise (`quiet`) is unseen until dismissed. */
  receiveBoxEgress: (e: BoxEgressEvent) => void;
  /** The user saw one workspace's blocked hosts: none of them prompts again. */
  dismissBoxEgress: (agentId: string) => void;
  /** Forget a blocked host — it was allowed. */
  clearBoxEgress: (host: string) => void;
  /** Stop one agent's box now; refusals go to `lastError`. */
  stopBox: (agentId: string) => Promise<void>;
}

/** One host a workspace's box was refused, across its connections. */
export interface BlockedHost {
  agent_id: string;
  host: string;
  port: number;
  /** Connections refused. */
  count: number;
  /** When the last one was, ms since the epoch. */
  lastAt: number;
  /** Known noise: telemetry and the like. Never prompts. */
  quiet: boolean;
  /** The user has seen it (or it is quiet): it does not prompt. */
  seen: boolean;
}

/** Most hosts kept: enough for any session, bounded for a box that spins. */
const MAX_BLOCKED = 200;

/** `blocked` with one more refused connection folded in: counted on its
 *  workspace and host when known, else added — unseen unless it is noise. */
export function foldBlocked(blocked: BlockedHost[], e: BoxEgressEvent): BlockedHost[] {
  const now = Date.now();
  const at = blocked.findIndex((b) => b.agent_id === e.agent_id && b.host === e.host);
  if (at >= 0) {
    const next = [...blocked];
    next[at] = { ...blocked[at], count: blocked[at].count + 1, lastAt: now, port: e.port };
    return next;
  }
  const quiet = e.quiet === true;
  const entry: BlockedHost = {
    agent_id: e.agent_id,
    host: e.host,
    port: e.port,
    count: 1,
    lastAt: now,
    quiet,
    seen: quiet,
  };
  return [...blocked, entry].slice(-MAX_BLOCKED);
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
  podmanProbe: null,
  containerBuilds: {},
  dockerImage: "",
  dockerMemory: "",
  dockerCpus: "",
  podmanImage: "",
  podmanMemory: "",
  podmanCpus: "",
  publishConfirmation: false,
  pendingPublishApprovals: [],
  boxesEnabled: false,
  boxMemoryMib: "",
  boxCpus: "",
  boxImageUrl: "",
  boxIdleStopMinutes: "",
  boxProbe: null,
  boxImage: null,
  boxTools: {},
  boxStates: {},
  boxEgressBlocked: [],

  setPublishConfirmation: async (enabled) => {
    await api.setPublishConfirmation(enabled);
    set({ publishConfirmation: enabled });
  },

  receivePublishApproval: (request) => {
    approvalsInFlight?.push({ kind: "requested", request });
    set((prev) => ({ pendingPublishApprovals: [...prev.pendingPublishApprovals, request] }));
  },

  loadPendingPublishApprovals: async () => {
    const env = activeEnvironment();
    if (env.kind !== "remote" || !hostSupports(env.protocol, "approvals_list")) return;
    const claim = approvalLoads.claim();
    const buffer: ApprovalEvent[] = [];
    approvalsInFlight = buffer;
    try {
      // Two guards, composing. A switch while the read was in flight owns the
      // queue now, and ours would be another host's prompts on this one's
      // screen; a newer load owns it too, and ours is the older snapshot it
      // was issued to replace.
      const pending = await forActiveEnvironment(() => api.listPublishApprovals());
      if (!pending || !claim.current()) return;
      // The snapshot is what the host was blocked on when it read its queue,
      // not when it answered; whatever it said in between is folded back in.
      set({ pendingPublishApprovals: replayApprovalEvents(pending, buffer) });
    } finally {
      if (approvalsInFlight === buffer) approvalsInFlight = null;
    }
  },

  answerPublishApproval: async (id, approved) => {
    // Drop it first: the prompt must not linger if the command throws, and a
    // request the backend has already timed out is a no-op there anyway.
    set((s) => ({
      pendingPublishApprovals: s.pendingPublishApprovals.filter((r) => r.id !== id),
    }));
    await api.answerPublishApproval(id, approved);
  },

  resolvePublishApproval: ({ id }) => {
    approvalsInFlight?.push({ kind: "resolved", id });
    set((s) => ({
      pendingPublishApprovals: s.pendingPublishApprovals.filter((r) => r.id !== id),
    }));
  },

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
  refreshPodmanProbe: async () => {
    let probe: PodmanProbe;
    try {
      probe = await api.probePodmanEngine();
    } catch {
      // A failed probe means we can't confirm podman — treat as not installed
      // so the option gates off rather than dangling enabled.
      probe = { status: "not-installed" };
    }
    set({ podmanProbe: probe });
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

  dismissDockerBuild: (runtime) =>
    set((s) => {
      const { [runtime]: _, ...rest } = s.containerBuilds;
      return { containerBuilds: rest };
    }),

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

  savePodmanLaunchSettings: (image, memory, cpus) =>
    optimistic(
      set,
      { podmanImage: image, podmanMemory: memory, podmanCpus: cpus },
      {
        podmanImage: get().podmanImage,
        podmanMemory: get().podmanMemory,
        podmanCpus: get().podmanCpus,
      },
      () => api.setPodmanLaunchSettings(image || null, memory || null, cpus || null),
    ),

  startDockerDesktop: async () => {
    try {
      await api.startDockerDesktop();
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  setBoxesEnabled: (enabled) =>
    optimistic(set, { boxesEnabled: enabled }, { boxesEnabled: get().boxesEnabled }, () =>
      api.setBoxSetting("boxes_enabled", String(enabled)),
    ),
  saveBoxSetting: async (key, value) => {
    try {
      await api.setBoxSetting(key, value || null);
      return null;
    } catch (e) {
      return String(e);
    }
  },
  refreshBoxProbe: async () => {
    try {
      const probe = await api.boxRuntimeProbe();
      // The probe is also the one read of the tools disks; `box:tools` carries
      // every change after.
      set({
        boxProbe: probe,
        boxTools: Object.fromEntries(probe.tools.map((t) => [t.provider, t])),
      });
    } catch {
      // A host that cannot answer (too old, or no box runtime) keeps whatever
      // was on screen; the probe is advisory.
    }
  },
  refreshBoxImage: async () => {
    try {
      set({ boxImage: await api.boxImageStatus() });
    } catch {
      // Same posture as the probe.
    }
  },
  installBoxImage: async () => {
    try {
      await api.boxImageInstall();
    } catch (e) {
      set({ lastError: String(e) });
    }
  },
  cancelBoxImage: async () => {
    try {
      await api.boxImageCancel();
    } catch (e) {
      set({ lastError: String(e) });
    }
  },
  retryBoxTools: async (provider) => {
    try {
      await api.boxToolsRetry(provider);
    } catch (e) {
      set({ lastError: String(e) });
    }
  },
  applyBoxImage: (e) => set({ boxImage: e }),
  applyBoxState: (e) => set((s) => ({ boxStates: { ...s.boxStates, [e.agent_id]: e } })),
  applyBoxTools: (e) => set((s) => ({ boxTools: { ...s.boxTools, [e.provider]: e } })),
  receiveBoxEgress: (e) => set((s) => ({ boxEgressBlocked: foldBlocked(s.boxEgressBlocked, e) })),
  dismissBoxEgress: (agentId) =>
    set((s) => ({
      boxEgressBlocked: s.boxEgressBlocked.map((b) =>
        b.agent_id === agentId ? { ...b, seen: true } : b,
      ),
    })),
  clearBoxEgress: (host) =>
    set((s) => ({ boxEgressBlocked: s.boxEgressBlocked.filter((b) => b.host !== host) })),
  stopBox: async (agentId) => {
    try {
      await api.boxStop(agentId);
    } catch (e) {
      set({ lastError: String(e) });
    }
  },
});
