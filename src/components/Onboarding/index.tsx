// Onboarding — a cinematic, native-feeling entry into Quorum. Full-screen
// overlay shown to new users on first launch and re-openable from Settings ›
// General. Ported from the design prototype (onboarding/app.jsx): ambient
// stage, step sequence, cinematic transitions, progress rail, keyboard nav.

import { useCallback, useEffect, useState } from "react";
import { useAppStore } from "../../store";
import { Icon } from "../Icon";
import { LANDMARK_NAMES } from "../../data/landmarks";
import { Ambient } from "./Ambient";
import { WelcomeStep, Beat, CreateStep, IgniteStep } from "./steps";
import { BEATS, REPOS } from "./beats";
import "./onboarding.css";

// flat step model: welcome · three feature beats · create · ignition
type Step =
  | { kind: "welcome" }
  | { kind: "beat"; beat: number }
  | { kind: "create" }
  | { kind: "ignite" };

const STEPS: Step[] = [
  { kind: "welcome" },
  { kind: "beat", beat: 0 },
  { kind: "beat", beat: 1 },
  { kind: "beat", beat: 2 },
  { kind: "create" },
  { kind: "ignite" },
];
const RAIL_LEN = 5; // welcome..create

/** Pick a landmark name at random, optionally avoiding `exclude`. */
function freeLandmark(exclude?: string): string {
  const pool = LANDMARK_NAMES.filter((n) => n !== exclude);
  const from = pool.length > 0 ? pool : LANDMARK_NAMES;
  return from[Math.floor(Math.random() * from.length)];
}

export function Onboarding() {
  const closeOnboarding = useAppStore((s) => s.closeOnboarding);

  const [idx, setIdx] = useState(0);
  const [phase, setPhase] = useState<"in" | "out">("in");
  const [ready, setReady] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);

  const [repo, setRepo] = useState(REPOS[0].full);
  const [agentName, setAgentName] = useState(() => freeLandmark());
  const [task, setTask] = useState("");

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

  const onAuth = (provider: string) => {
    if (busy) return;
    setBusy(provider);
    window.setTimeout(() => {
      setBusy(null);
      go(1);
    }, 1100);
  };

  const reroll = () => setAgentName((n) => freeLandmark(n));
  const onCreate = () => go(STEPS.length - 1);
  const onEnter = () => closeOnboarding();

  // keyboard navigation
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement | null)?.tagName;
      const inField = tag === "TEXTAREA" || tag === "INPUT";
      if (e.key === "Escape") {
        closeOnboarding();
      } else if (e.key === "Enter" && step.kind === "welcome" && !busy) {
        onAuth("github");
      } else if ((e.key === "ArrowRight" || e.key === "Enter") && step.kind === "beat" && !inField) {
        e.preventDefault();
        next();
      } else if (e.key === "ArrowLeft" && idx > 0 && step.kind !== "ignite" && !inField) {
        back();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idx, busy, step.kind, next, back]);

  const projName = (REPOS.find((r) => r.full === repo) || REPOS[0]).full.split("/")[1];

  let content = null;
  if (step.kind === "welcome") content = <WelcomeStep onAuth={onAuth} busy={busy} />;
  else if (step.kind === "beat") content = <Beat beat={BEATS[step.beat]} />;
  else if (step.kind === "create")
    content = (
      <CreateStep
        repo={repo}
        setRepo={setRepo}
        agentName={agentName}
        reroll={reroll}
        task={task}
        setTask={setTask}
        onCreate={onCreate}
      />
    );
  else if (step.kind === "ignite")
    content = <IgniteStep agentName={agentName} projName={projName} onEnter={onEnter} />;

  const showFoot = step.kind !== "ignite";
  const showBack = idx > 0 && step.kind !== "ignite";
  const showNext = step.kind === "beat";

  return (
    <div className="ob">
      <div className="ob-tb" data-tauri-drag-region>
        <div className="ob-tb-gutter" data-tauri-drag-region />
        <div className="ob-tb-mark">
          <span className="d" />
          <span>QUORUM</span>
        </div>
        <div className="ob-tb-right">
          {step.kind !== "ignite" && (
            <span className="ob-step-count">
              <b>{Math.min(idx + 1, RAIL_LEN)}</b> / {RAIL_LEN}
            </span>
          )}
          {step.kind !== "ignite" && step.kind !== "create" && (
            <button className="ob-skip" onClick={() => go(STEPS.length - 2)}>
              Skip
            </button>
          )}
          <button className="ob-close" title="Close (Esc)" aria-label="Close onboarding" onClick={() => closeOnboarding()}>
            <Icon name="close" size={15} />
          </button>
        </div>
      </div>

      <div className="ob-stage">
        <Ambient phase={idx} />

        <div className={`ob-view ${ready ? "ready" : ""} ${phase === "out" ? "out" : ""}`} key={idx}>
          {content}
        </div>

        {showFoot && (
          <div className="ob-foot">
            <div className="ob-foot-l">
              {showBack && (
                <button className="ob-back" onClick={back}>
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
                <button className="ob-next" onClick={next}>
                  Continue <Icon name="arrowR" />
                </button>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
