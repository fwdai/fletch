import { invoke } from "../invoke";
import type { DetectedEditor } from "../types/providers";

export const miscApi = {
  revealLogs: () => invoke<void>("reveal_logs"),
  /** Editors installed on this machine, in picker order. */
  detectEditors: () => invoke<DetectedEditor[]>("detect_editors"),
  /** Open an agent's checkout in the chosen editor. */
  openInEditor: (agentId: string, editorId: string) =>
    invoke<void>("open_in_editor", { agentId, editorId }),
  // Anonymous usage telemetry. Persists the opt-out flag and toggles the live
  // pipeline (events themselves are emitted from the backend).
  setTelemetryEnabled: (enabled: boolean) => invoke<void>("set_telemetry_enabled", { enabled }),
  // Emit the deferred first `app_opened` once onboarding completes — i.e. after
  // the data-sharing disclosure has been shown. See `track_app_opened` (Rust).
  trackAppOpened: () => invoke<void>("track_app_opened"),
};
