// Renderer-side product telemetry. One typed gate over the backend's
// `track_event` command, so every frontend event has a declared name and a
// declared property shape — the compiler is what stops an ad-hoc
// `track("thing", { repoPath })` from leaking a path into PostHog.
//
// Scope: the onboarding funnel and the activation-path moments only the
// frontend can see. Backend-observable events (`app_opened`, `agent_spawned`,
// `pr_opened`, `turn_completed`) are emitted from Rust and are not repeated
// here — one event, one emitter.
//
// Consent is enforced downstream in `telemetry::track` (Rust), so nothing here
// needs to check the opt-out flag. Properties stay categorical — enums, counts,
// booleans; never paths, repo/branch names, prompts, or raw error strings.

import { api } from "@/api";
import type { ConnectFailureReason } from "./connectFailure";

/** The onboarding overlay's two screens (mirrors `Onboarding/index.tsx`). */
export type OnboardingStep = "welcome" | "preflight";

/** The pre-flight check's rows, each of which the user can decline (mirrors
 *  `Onboarding/Preflight/usePreflight.ts`). */
export type OnboardingCheck = "agents" | "git" | "github";

/** Where a GitHub device-flow sign-in was launched from. Lets us tell an
 *  onboarding sign-in (funnel drop-off) from a later one (a connect gate the
 *  user hit mid-task). */
export type ConnectSource = "onboarding_github" | "new_project" | "settings" | "connect_gate";

/** How a project entered the sidebar. */
export type ProjectAddMethod = "existing" | "clone" | "create";

/** Carried by every onboarding event. Onboarding is replayable from Settings ›
 *  Developer, and a replay by an existing user is not a new-user funnel — so
 *  every step of a PostHog funnel must be filtered on `first_run = true` for
 *  the drop-off numbers to mean anything.
 *
 *  Declared as a type alias, not an interface: only aliases get the implicit
 *  index signature that makes them assignable to the `Record<string, unknown>`
 *  the IPC boundary takes. */
type OnboardingCommon = {
  first_run: boolean;
};

/** Every renderer-emitted event, with its exact property shape. Adding an
 *  event means adding a line here first.
 *
 *  One event is deliberately not here: `feedback_submitted` (see
 *  `docs/feedback.md`). It goes through `api.submitFeedback`, because it must be
 *  awaited to report failure, must ignore the telemetry consent gate, and
 *  carries a screenshot — none of which `track`'s fire-and-forget contract
 *  covers. */
interface EventMap {
  // ── onboarding funnel ────────────────────────────────────────────────────
  /** A screen came on screen. The whole drop-off curve is derivable from this. */
  onboarding_step_viewed: OnboardingCommon & { step: OnboardingStep; index: number };
  /** A pre-flight row the user declined ("I'll install one later", "Not now")
   *  — the only way to pass a row that needs them. */
  onboarding_step_skipped: OnboardingCommon & { step: OnboardingCheck };
  /** The ✕ before "Open Fletch" — the "gave up here" signal. */
  onboarding_abandoned: OnboardingCommon & { step: OnboardingStep };
  /** What the pre-flight found on this machine, once per onboarding session,
   *  the moment the automatic checks have an answer — on whichever screen the
   *  user is, so a machine whose owner quits at the welcome is still counted.
   *  The one event that says what people install Fletch onto: which agent
   *  CLIs they already have (by provider id), whether git was theirs, ours,
   *  or missing, and whether GitHub was already connected. `os`, `arch` and
   *  `app_version` ride along on every event from the backend. */
  environment_detected: OnboardingCommon & {
    /** Provider ids of the agent CLIs found on PATH, e.g. ["claude", "codex"]. */
    agents: string[];
    agents_detected: number;
    /** `system`: the user's own git; `portable`: the copy Fletch downloaded;
     *  `none`: neither usable at the time of the check. */
    git: "system" | "portable" | "none";
    /** Major.minor only, e.g. "2.47" — enough to spot an old git, bounded. */
    git_version: string | null;
    gh_connected: boolean;
  };
  /** "Open Fletch", with what the user actually finished with. The pre-flight
   *  never blocks, so the flags are the point. */
  onboarding_completed: OnboardingCommon & {
    git_ready: boolean;
    gh_connected: boolean;
    agents_detected: number;
  };

  // ── GitHub device flow ───────────────────────────────────────────────────
  // started + one terminal event, so the browser round-trip (the step most
  // likely to strand someone) has a measurable completion rate. `reason` on a
  // failure is a category (`connectFailure.ts`), never the error text.
  github_connect_started: { source: ConnectSource; provider: string };
  github_connect_succeeded: { source: ConnectSource; provider: string };
  github_connect_failed: { source: ConnectSource; provider: string; reason: ConnectFailureReason };
  github_connect_cancelled: { source: ConnectSource; provider: string };

  // ── portable git bootstrap ───────────────────────────────────────────────
  git_install_started: Record<string, never>;
  git_install_succeeded: Record<string, never>;
  git_install_failed: Record<string, never>;

  // ── one-click agent CLI install ──────────────────────────────────────────
  agent_cli_install_started: { provider: string };
  agent_cli_install_succeeded: { provider: string };
  agent_cli_install_failed: { provider: string };

  // ── activation ───────────────────────────────────────────────────────────
  /** A project landed in the sidebar. `first` marks the activation moment. */
  project_added: { method: ProjectAddMethod; first: boolean };
}

export type TrackedEvent = keyof EventMap;

/** Record a product event. Fire-and-forget: telemetry must never break a user
 *  flow, so a failed IPC is swallowed. */
export function track<E extends TrackedEvent>(event: E, props: EventMap[E]): void {
  void api.trackEvent(event, props).catch(() => {});
}
