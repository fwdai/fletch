// The box sandbox (docs/boxes-spec.md §9). Every call follows the active
// environment: the probe, the image and a box live on the host that runs the
// agents. The five `box_*` settings are host-owned and written only through
// `setBoxSetting` (never `setSetting`), since the engine reads them.
import { invoke } from "../invoke";
import type {
  BoxImageStatus,
  BoxRuntimeProbe,
  BoxSettingKey,
  BoxStateEvent,
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
  /** The registry's phase for one agent's box. */
  boxState: (agentId: string) => invoke<BoxStateEvent>("box_state", { agentId }),
  /** Stop the agent's box now; resolves to which rung of the ladder worked. */
  boxStop: (agentId: string) => invoke<string>("box_stop", { agentId }),
  /** Add one host pattern to a project's egress allow-list; resolves to the
   *  list as stored. */
  boxEgressAllow: (projectId: string, pattern: string) =>
    invoke<string[]>("box_egress_allow", { projectId, pattern }),
  /** Write one validated box setting; `null` restores its default. Rejects
   *  with the validation message. */
  setBoxSetting: (key: BoxSettingKey, value: string | null) =>
    invoke<void>("set_box_setting", { key, value }),
};
