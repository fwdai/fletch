// Onboarding — a cinematic, native-feeling entry into Fletch. Full-screen
// overlay shown to new users on first launch and re-openable from Settings ›
// General. The sequence is functional, not a tour: sign in, then each step
// verifies (and can fix) one requirement — Git installed, GitHub connected,
// an agent CLI present — before the handoff. Ambient stage, cinematic
// transitions, progress rail, and keyboard nav carry over from the original
// tour; the exhibits now sit beside real controls.

import { useCallback, useEffect, useState } from "react";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
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

  const [idx, setIdx] = useState(0);
  const [phase, setPhase] = useState<"in" | "out">("in");
  const [ready, setReady] = useState(false);

  const step = STEPS[idx];

  // Trigger the staggered reveal after the step mounts / changes. setTimeout
  // (not rAF) so it still fires when the window is backgrounded.
  useEffect(() => {
    setReady(false);
    const id = window.setTimeout(() => setReady(true), 40);
    return () => window.clearTimeout(id);
  }, [idx]);

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
  } = useGithubConnect(onSignedIn);
  const onAuth = useCallback((provider: string) => void connect(provider), [connect]);

  // Handoff: just drop into the real app. Its empty state prompts the user to
  // add their first repo from the sidebar — no auto-picker.
  const onEnter = useCallback(() => closeOnboarding(), [closeOnboarding]);

  // keyboard navigation
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement | null)?.tagName;
      const inField = tag === "TEXTAREA" || tag === "INPUT";
      if (e.key === "Escape") {
        closeOnboarding();
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
  }, [idx, busy, step, showNext, canContinue, next, back]);

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
  else if (step === "github") content = <GithubStep setup={setup} onSkip={next} />;
  else if (step === "agents") content = <AgentsStep setup={setup} onSkip={next} />;
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
            <button className="ob-skip text-sm" onClick={() => go(STEPS.length - 1)}>
              Skip
            </button>
          )}
          <button
            className="ob-close"
            title="Close (Esc)"
            aria-label="Close onboarding"
            onClick={() => closeOnboarding()}
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
