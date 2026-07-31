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
  // Code indexing (codegraph). Persists the flag and, when enabled, warms the
  // index in the background (install + per-repo mirror). Backend-owned.
  setCodeIndexingEnabled: (enabled: boolean) =>
    invoke<void>("set_code_indexing_enabled", { enabled }),
  // Emit the deferred first `app_opened` once the onboarding overlay (which
  // carries the data-sharing disclosure) is on screen. Idempotent per process.
  // See `track_app_opened` (Rust).
  trackAppOpened: () => invoke<void>("track_app_opened"),
  // Raise a renderer-observed product event. Don't call this directly — go
  // through the typed `track()` in `@/util/track`, which is the gate that keeps
  // event names and property shapes honest.
  trackEvent: (event: string, props: Record<string, unknown>) =>
    invoke<void>("track_event", { event, props }),
  /** Send one piece of user feedback (sidebar footer → feedback modal). Not a
   *  `trackEvent`: feedback is consent-independent, awaited, and carries a
   *  screenshot, so it has its own command. Rejects with a presentable message
   *  when the send fails, so the modal can offer its mailto fallback instead of
   *  faking success. See docs/feedback.md. */
  submitFeedback: (p: {
    message: string;
    contactEmail?: string | null;
    /** Base64 JPEG, already downscaled by `util/image.ts`. */
    screenshotBase64?: string | null;
    source?: string;
  }) => invoke<void>("submit_feedback", p),
};
