import {
  api,
  type BoxEgressEvent,
  type BoxImageStatus,
  type BoxRuntimeProbe,
  type BoxSettingKey,
  type BoxStateEvent,
  type BoxToolsStatus,
  type ContainerAuthStatus,
  type PublishApproval,
  type PublishApprovalResolved,
} from "@/api";
import { hostSupports } from "@/remote/types";
import { newestWins } from "@/util/newestWins";
import { type ApprovalEvent, replayApprovalEvents } from "@/util/publishApprovals";
import { activeEnvironment, forActiveEnvironment } from "./environments";
import type { SliceCreator } from "./types";

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

export interface SandboxSlice {
  /** Which claude auth chain step is active (Anthropic credentials for box
   *  agents); `null` until the first refresh lands. */
  containerAuth: ContainerAuthStatus | null;
  /** Whether an agent must get the user's approval before publishing. Mirrors
   *  the backend-owned `publish_confirmation` setting. Off by default. */
  publishConfirmation: boolean;
  /** Publishes waiting on the user, oldest first. Two agents can be waiting at
   *  once, so this is a queue rather than a single slot; the prompt shows the
   *  head. Unanswered requests are denied backend-side after a timeout, so a
   *  dropped entry fails closed. */
  pendingPublishApprovals: PublishApproval[];
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
  /** Re-resolve the claude auth chain into `containerAuth`. */
  refreshContainerAuth: () => Promise<void>;
  /** Persist a pasted `claude setup-token` for boxes, then refresh the
   *  status. Throws on backend refusal (empty token) so the connect modal
   *  can show the error inline. */
  setContainerAuthToken: (token: string) => Promise<void>;
  /** Drop the stored setup token, then refresh the status (a later chain
   *  step — shell env / credentials file — may take over). */
  clearContainerAuthToken: () => Promise<void>;
  /** Read where the host's box setup stands — the runtime probe (with the
   *  tools disks) and the image — once per environment; `box:image` and
   *  `box:tools` carry every change after. What the title bar's popover,
   *  the ready toast and the onboarding note read. */
  loadBoxSetup: () => Promise<void>;
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

export const createSandboxSlice: SliceCreator<SandboxSlice> = (set, get) => ({
  publishConfirmation: false,
  pendingPublishApprovals: [],
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
      // Bound to the environment that was asked: an answer that lands after
      // a switch is another host's and is dropped, as every host read is.
      const probe = await forActiveEnvironment(() => api.boxRuntimeProbe());
      if (!probe) return;
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
      const image = await forActiveEnvironment(() => api.boxImageStatus());
      if (image) set({ boxImage: image });
    } catch {
      // Same posture as the probe.
    }
  },
  loadBoxSetup: async () => {
    // The setup is the active host's, wholesale: what the last one said is
    // cleared before this one answers, so a switch never shows host A's
    // download on host B.
    set({ boxProbe: null, boxImage: null, boxTools: {} });
    await Promise.all([get().refreshBoxProbe(), get().refreshBoxImage()]);
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
