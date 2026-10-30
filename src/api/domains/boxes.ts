// The box sandbox. Every call follows the active
// environment: the probe, the image and a box live on the host that runs the
// agents. The five `box_*` settings are host-owned and written only through
// `setBoxSetting` (never `setSetting`), since the engine reads them.
import { invoke } from "../invoke";
import type {
  BoxImageStatus,
  BoxRuntimeProbe,
  BoxSettingKey,
  BoxStateEvent,
  EgressDelta,
  EgressOverview,
} from "../types/sandbox";

export const boxesApi = {
  /** Can this host boot a box: helper, dylibs, hypervisor, arch, image, flag. */
  boxRuntimeProbe: () => invoke<BoxRuntimeProbe>("box_runtime_probe"),
  /** The pinned image's install state; `onBoxImage` carries the changes. */
  boxImageStatus: () => invoke<BoxImageStatus>("box_image_status"),
  /** Start installing the pinned image in the background. Refused while one
   *  is running, or with the flag off. */
  boxImageInstall: () => invoke<void>("box_image_install"),
  boxImageCancel: () => invoke<void>("box_image_cancel"),
  /** Build a provider's tools disk now, past the backoff that holds builds
   *  off after failures; the outcome arrives as `onBoxTools`. */
  boxToolsRetry: (provider: string) => invoke<void>("box_tools_retry", { provider }),
  /** The registry's phase for one agent's box. */
  boxState: (agentId: string) => invoke<BoxStateEvent>("box_state", { agentId }),
  /** Stop the agent's box now; resolves to which rung of the ladder worked. */
  boxStop: (agentId: string) => invoke<string>("box_stop", { agentId }),
  /** Add one host pattern to a project's egress allow-list; resolves to the
   *  list as stored. */
  boxEgressAllow: (projectId: string, pattern: string) =>
    invoke<string[]>("box_egress_allow", { projectId, pattern }),
  /** The egress allow-list layer by layer; with a project, its own too. */
  boxEgressOverview: (projectId?: string) =>
    invoke<EgressOverview>("box_egress_overview", { projectId: projectId ?? null }),
  /** Replace the user's global changes over the template; resolves to what
   *  was stored. Rejects a pattern that does not parse. */
  setBoxEgressGlobal: (delta: EgressDelta) =>
    invoke<EgressDelta>("set_box_egress_global", { delta }),
  /** Write one validated box setting; `null` restores its default. Rejects
   *  with the validation message. */
  setBoxSetting: (key: BoxSettingKey, value: string | null) =>
    invoke<void>("set_box_setting", { key, value }),
};
