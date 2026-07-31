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

/** The onboarding overlay's flat step model (mirrors `Onboarding/index.tsx`). */
export type OnboardingStep = "welcome" | "git" | "github" | "agents" | "ready";

/** Where a GitHub device-flow sign-in was launched from. Lets us tell an
 *  onboarding sign-in (funnel drop-off) from a later one (a connect gate the
 *  user hit mid-task). */
export type ConnectSource =
  | "onboarding_welcome"
  | "onboarding_github"
  | "new_project"
  | "settings"
  | "connect_gate";

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
 *  event means adding a line here first. */
interface EventMap {
  // ── onboarding funnel ────────────────────────────────────────────────────
  /** A step came on screen. The whole drop-off curve is derivable from this. */
  onboarding_step_viewed: OnboardingCommon & { step: OnboardingStep; index: number };
  /** A per-step opt-out ("I use GitLab…", "Set up later"). */
  onboarding_step_skipped: OnboardingCommon & { step: OnboardingStep };
  /** The title-bar Skip, which jumps straight to the handoff. */
  onboarding_skipped: OnboardingCommon & { step: OnboardingStep };
  /** Esc / ✕ before reaching the handoff — the "gave up here" signal. */
  onboarding_abandoned: OnboardingCommon & { step: OnboardingStep };
  /** "Enter Fletch", with what the user actually finished with. Reachable with
   *  gaps via Skip, so the flags matter. */
  onboarding_completed: OnboardingCommon & {
    git_ready: boolean;
    gh_connected: boolean;
    agents_detected: number;
  };

  // ── GitHub device flow ───────────────────────────────────────────────────
  // started + one terminal event, so the browser round-trip (the step most
  // likely to strand someone) has a measurable completion rate.
  github_connect_started: { source: ConnectSource; provider: string };
  github_connect_succeeded: { source: ConnectSource; provider: string };
  github_connect_failed: { source: ConnectSource; provider: string };
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
