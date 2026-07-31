// Onboarding — a cinematic, native-feeling entry into Fletch. Full-screen
// overlay shown to new users on first launch and re-openable from Settings ›
// General. The sequence is functional, not a tour: sign in, then each step
// verifies (and can fix) one requirement — Git installed, GitHub connected,
// an agent CLI present — before the handoff. Ambient stage, cinematic
// transitions, progress rail, and keyboard nav carry over from the original
// tour; the exhibits now sit beside real controls.

import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "@/api";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { track } from "@/util/track";
import { useGithubConnect } from "@/util/useGithubConnect";
import { AgentsStep } from "./AgentsStep";
import { Ambient } from "./Ambient";
import { DeviceCode } from "./DeviceCode";
import { GithubStep } from "./GithubStep";
import { GitStep } from "./GitStep";
import { ReadyStep } from "./ReadyStep";
import { WelcomeStep } from "./steps";
import { useOnboardingSetup } from "./useSetup";
import "./onboarding.css";

// flat step model: sign-in, three requirement steps, handoff
type StepKind = "welcome" | "git" | "github" | "agents" | "ready";

const STEPS: StepKind[] = ["welcome", "git", "github", "agents", "ready"];
const RAIL_LEN = 4; // welcome..agents (the ready handoff is excluded)

export function Onboarding() {
  const closeOnboarding = useAppStore((s) => s.closeOnboarding);
  const onboardingComplete = useAppStore((s) => s.onboardingComplete);

  const [idx, setIdx] = useState(0);
  const [phase, setPhase] = useState<"in" | "out">("in");
  const [ready, setReady] = useState(false);

  // Is this the real first run, or a replay from Settings › Developer? Frozen
  // at mount: `closeOnboarding` flips `onboardingComplete` to true, and the
  // terminal event must not report itself as a replay on the way out.
  const firstRun = useRef(!onboardingComplete).current;

  const step = STEPS[idx];

  // Trigger the staggered reveal after the step mounts / changes. setTimeout
  // (not rAF) so it still fires when the window is backgrounded.
  useEffect(() => {
    setReady(false);
    const id = window.setTimeout(() => setReady(true), 40);
    return () => window.clearTimeout(id);
  }, [idx]);

  // On a fresh install the backend defers the first `app_opened` to here: the
  // welcome step carries the data-sharing disclosure, so this is the earliest
  // point at which telemetry has been disclosed. Firing it on mount (rather
  // than at completion, as before) is what makes the drop-off events below
  // meaningful — a user who quits at step 02 is exactly who we want to see.
  //
  // Only on a real first run: an already-onboarded user got their `app_opened`
  // at launch, so a replay must not send a second one. (`send_app_opened` in
  // Rust also collapses repeats, which covers StrictMode's double-mount.)
  useEffect(() => {
    if (firstRun) void api.trackAppOpened();
  }, [firstRun]);

  // The funnel. One event per step reveal gives the whole drop-off curve;
  // the skip/abandon/complete events below say *how* each user left.
  useEffect(() => {
    track("onboarding_step_viewed", { step: STEPS[idx], index: idx, first_run: firstRun });
  }, [idx, firstRun]);

  const go = useCallback(
    (next: number) => {
      if (next < 0 || next >= STEPS.length || next === idx) return;
      setPhase("out");
      window.setTimeout(() => {
        setIdx(next);
        setPhase("in");
        const stage = document.querySelector(".ob-view");
        if (stage) stage.scrollTop = 0;
      }, 380);
    },
    [idx],
  );

  const next = useCallback(() => go(idx + 1), [go, idx]);
  const back = useCallback(() => go(idx - 1), [go, idx]);

  // Shared requirement checks: the steps render/fix them, the footer gates
  // Continue on them. Agent probing polls while the agents step is up so a
  // finished install lights up without a manual re-check.
  const setup = useOnboardingSetup(step === "agents");

  // Per-step gate for the footer's Continue. The welcome and ready steps have
  // their own primary actions (sign-in / Enter Fletch) instead.
  const canContinue =
    step === "git"
      ? setup.gitReady
      : step === "github"
        ? setup.ghConnected
        : step === "agents"
          ? setup.hasAgent
          : false;
  const showNext = step === "git" || step === "github" || step === "agents";

  // Exactly one terminal event per onboarding session, whichever exit the user
  // takes: Esc, ✕, and "Enter Fletch" all funnel through `leave`. A ref (not
  // state) guards it, so a second call during the closing render can't
  // double-count. `completed` carries what the user actually finished with —
  // the handoff is reachable with gaps via Skip, so the flags are the point.
  const leftRef = useRef(false);
  const leave = useCallback(
    (reason: "completed" | "abandoned") => {
      if (!leftRef.current) {
        leftRef.current = true;
        if (reason === "completed") {
          track("onboarding_completed", {
            git_ready: setup.gitReady,
            gh_connected: setup.ghConnected,
            agents_detected: setup.detected,
            first_run: firstRun,
          });
        } else {
          track("onboarding_abandoned", { step, first_run: firstRun });
        }
      }
      closeOnboarding();
    },
    [closeOnboarding, step, firstRun, setup.gitReady, setup.ghConnected, setup.detected],
  );

  // Shared device-flow sign-in. On success advance off the welcome step; the
  // hook persists the profile and refreshes the account + GitHub connection.
  // The requirement checks re-run so a GitHub sign-in pre-passes step 02.
  const onSignedIn = useCallback(() => {
    setup.recheck();
    go(1);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [go, setup.recheck]);
  const {
    connect,
    cancel: cancelAuth,
    device,
    error: authError,
    busy,
  } = useGithubConnect(onSignedIn, "onboarding_welcome");
  const onAuth = useCallback((provider: string) => void connect(provider), [connect]);

  // Handoff: just drop into the real app. Its empty state prompts the user to
  // add their first repo from the sidebar — no auto-picker.
  const onEnter = useCallback(() => leave("completed"), [leave]);

  // A step's own opt-out ("I use GitLab…", "Set up later"), distinct from the
  // title-bar Skip: it declines one requirement rather than the whole flow.
  const skipStep = useCallback(() => {
    track("onboarding_step_skipped", { step, first_run: firstRun });
    next();
  }, [next, step, firstRun]);

  // keyboard navigation
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement | null)?.tagName;
      const inField = tag === "TEXTAREA" || tag === "INPUT";
      if (e.key === "Escape") {
        leave(step === "ready" ? "completed" : "abandoned");
      } else if (e.key === "Enter" && step === "welcome" && !busy) {
        onAuth("github");
      } else if ((e.key === "ArrowRight" || e.key === "Enter") && showNext && !inField) {
        if (!canContinue) return;
        e.preventDefault();
        next();
      } else if (e.key === "ArrowLeft" && idx > 0 && !inField) {
        back();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idx, busy, step, showNext, canContinue, next, back, leave]);

  let content = null;
  if (step === "welcome")
    // The only way off welcome is a successful sign-in (go(1)), so during its
    // fade-out (`phase === "out"`) keep the device panel up rather than
    // flashing the sign-in buttons as the hook clears its state.
    content =
      busy || device || authError || phase === "out" ? (
        <DeviceCode info={device} error={authError} onCancel={cancelAuth} />
      ) : (
        <WelcomeStep onAuth={onAuth} busy={busy} />
      );
  else if (step === "git") content = <GitStep setup={setup} />;
  else if (step === "github") content = <GithubStep setup={setup} onSkip={skipStep} />;
  else if (step === "agents") content = <AgentsStep setup={setup} onSkip={skipStep} />;
  else if (step === "ready") content = <ReadyStep setup={setup} onEnter={onEnter} />;

  const showBack = idx > 0;

  return (
    <div className="ob">
      <div className="ob-tb" data-tauri-drag-region>
        <div className="ob-tb-gutter" data-tauri-drag-region />
        <div className="ob-tb-mark text-xs">
          <span className="d" />
          <span>FLETCH</span>
        </div>
        <div className="ob-tb-right">
          {step !== "ready" && (
            <span className="ob-step-count text-xs">
              <b>{Math.min(idx + 1, RAIL_LEN)}</b> / {RAIL_LEN}
            </span>
          )}
          {step !== "ready" && (
            <button
              className="ob-skip text-sm"
              onClick={() => {
                track("onboarding_skipped", { step, first_run: firstRun });
                go(STEPS.length - 1);
              }}
            >
              Skip
            </button>
          )}
          <button
            className="ob-close"
            title="Close (Esc)"
            aria-label="Close onboarding"
            onClick={() => leave(step === "ready" ? "completed" : "abandoned")}
          >
            <Icon name="close" size={15} />
          </button>
        </div>
      </div>

      <div className="ob-stage">
        <Ambient phase={idx} />

        <div
          className={`ob-view ${ready ? "ready" : ""} ${phase === "out" ? "out" : ""}`}
          key={idx}
        >
          {content}
        </div>

        <div className="ob-foot">
          <div className="ob-foot-l">
            {showBack && (
              <button className="ob-back text-base" onClick={back}>
                <Icon name="chevL" /> Back
              </button>
            )}
          </div>
          <div className="ob-rail">
            {Array.from({ length: RAIL_LEN }).map((_, i) => (
              <span
                key={i}
                className={`seg ${i < idx ? "done" : ""} ${i === idx ? "cur" : ""}`}
                onClick={() => {
                  if (i <= idx) go(i);
                }}
              />
            ))}
          </div>
          <div className="ob-foot-r">
            {showNext && (
              <button
                className="ob-next text-base"
                onClick={next}
                disabled={!canContinue}
                title={canContinue ? undefined : "Complete this step to continue"}
              >
                Continue <Icon name="arrowR" />
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
