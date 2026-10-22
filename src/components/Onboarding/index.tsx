// Onboarding — two screens and out. A welcome, then a pre-flight check that
// plays through on its own, then straight into the app: no sign-in, no tour,
// no summary. Full-screen overlay shown on first launch and re-openable from
// Settings › Developer. The requirement checks start the moment this mounts,
// so they are usually answered before the welcome is even dismissed.

import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "@/api";
import { FletchLockup } from "@/components/FletchLockup";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { track } from "@/util/track";
import { Ambient } from "./Ambient";
import { Preflight } from "./Preflight";
import { preflightSettled } from "./Preflight/usePreflight";
import { useOnboardingSetup } from "./useSetup";
import { Welcome } from "./Welcome";
import "./onboarding.css";

type Screen = "welcome" | "preflight";
const SCREENS: Screen[] = ["welcome", "preflight"];

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

  const screen = SCREENS[idx];

  // Trigger the staggered reveal after the screen mounts / changes. setTimeout
  // (not rAF) so it still fires when the window is backgrounded.
  useEffect(() => {
    setReady(false);
    const id = window.setTimeout(() => setReady(true), 40);
    return () => window.clearTimeout(id);
  }, [idx]);

  // On a fresh install the backend defers the first `app_opened` to here: the
  // welcome carries the data-sharing disclosure, so this is the earliest point
  // at which telemetry has been disclosed. Only on a real first run: an
  // already-onboarded user got their `app_opened` at launch. (`send_app_opened`
  // in Rust also collapses repeats, which covers StrictMode's double-mount.)
  useEffect(() => {
    if (firstRun) void api.trackAppOpened();
  }, [firstRun]);

  // The funnel: one event per screen, plus the terminal event below.
  useEffect(() => {
    track("onboarding_step_viewed", { step: SCREENS[idx], index: idx, first_run: firstRun });
  }, [idx, firstRun]);

  const go = useCallback(
    (next: number) => {
      if (next < 0 || next >= SCREENS.length || next === idx) return;
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
  const start = useCallback(() => go(1), [go]);

  // The checks run from mount; agent probing polls while the pre-flight is up
  // so an install finishing lights its row up without a manual re-check.
  const setup = useOnboardingSetup(screen === "preflight");
  const settled = preflightSettled(setup);

  // What this machine has, reported once the checks have an answer — on
  // whichever screen that happens. A ref guards it: `settled` can flip back
  // while a git download runs, and a replay must not report twice either.
  const reportedRef = useRef(false);
  useEffect(() => {
    if (!settled || reportedRef.current) return;
    reportedRef.current = true;
    const agents = setup.agents.filter((p) => !!setup.providerPaths[p.id]).map((p) => p.id);
    const version = setup.git?.version?.match(/^\d+\.\d+/)?.[0] ?? null;
    track("environment_detected", {
      agents,
      agents_detected: agents.length,
      git: setup.git?.installed ? (setup.git.source ?? "system") : "none",
      git_version: setup.git?.installed ? version : null,
      gh_connected: setup.ghConnected,
      first_run: firstRun,
    });
  }, [settled, setup, firstRun]);

  // Exactly one terminal event per onboarding session, whichever exit the user
  // takes: the ✕ and "Open Fletch" both funnel through `leave`. A ref (not
  // state) guards it, so a second call during the closing render can't
  // double-count. `completed` carries what the user actually finished with —
  // the pre-flight never blocks, so the flags are the point.
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
          track("onboarding_abandoned", { step: screen, first_run: firstRun });
        }
      }
      closeOnboarding();
    },
    [closeOnboarding, screen, firstRun, setup.gitReady, setup.ghConnected, setup.detected],
  );
  const finish = useCallback(() => leave("completed"), [leave]);

  // Enter starts from the welcome (the pre-flight binds its own, once it has
  // something to open). Deliberately no Escape binding: closing is a one-way
  // door (it persists `onboardingComplete`, and only Settings › Developer can
  // reopen it), so a stray Esc must not end first-run setup. The ✕ is always
  // visible.
  useEffect(() => {
    if (screen !== "welcome") return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Enter") start();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [screen, start]);

  return (
    <div className="ob">
      <div className="ob-tb" data-tauri-drag-region>
        <div className="ob-tb-gutter" data-tauri-drag-region />
        <FletchLockup className="ob-tb-brand" />
        <div className="ob-tb-right">
          <button
            type="button"
            className="ob-close"
            title="Close onboarding"
            aria-label="Close onboarding"
            onClick={() => leave("abandoned")}
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
          {screen === "welcome" ? (
            <Welcome onStart={start} />
          ) : (
            <Preflight setup={setup} firstRun={firstRun} onFinish={finish} />
          )}
        </div>
      </div>
    </div>
  );
}
