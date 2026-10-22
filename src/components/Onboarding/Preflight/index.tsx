// Screen 2 · Pre-flight. A timeline of three rows on the left — agents, git,
// GitHub — that plays through at a readable pace and stops only where a
// person is needed, beside a panel on the right showing the work as it
// happens. "Open Fletch" appears once every row has its answer, so the user
// is never offered two ways forward at once.

import { type CSSProperties, useEffect } from "react";
import { track } from "@/util/track";
import { useGithubConnect } from "@/util/useGithubConnect";
import { ExhibitCode, ExhibitParallel } from "../exhibits";
import type { OnboardingSetup } from "../useSetup";
import { AgentsPanel } from "./AgentsPanel";
import { AgentsRow } from "./AgentsRow";
import { GithubRow } from "./GithubRow";
import { GitRow } from "./GitRow";
import { CHECKS, type CheckId, usePreflight } from "./usePreflight";

export function Preflight({
  setup,
  firstRun,
  onFinish,
}: {
  setup: OnboardingSetup;
  firstRun: boolean;
  onFinish: () => void;
}) {
  // A successful sign-in re-runs the checks so the GitHub row turns green.
  const github = useGithubConnect(setup.recheck, "onboarding_github");
  const pf = usePreflight(setup, github.busy);

  // Enter opens the app once it is offered. Buttons and fields keep Enter
  // for themselves (a focused "Not now" must not open the app instead).
  useEffect(() => {
    if (!pf.done) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Enter") return;
      const tag = (e.target as HTMLElement | null)?.tagName;
      if (tag === "TEXTAREA" || tag === "INPUT" || tag === "BUTTON") return;
      onFinish();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [pf.done, onFinish]);

  const skip = (id: CheckId) => {
    track("onboarding_step_skipped", { step: id, first_run: firstRun });
    pf.skip(id);
  };
  const rowProps = (id: CheckId) => {
    const i = CHECKS.indexOf(id);
    return {
      state: pf.states[id],
      active: pf.active === i,
      onInspect: () => pf.inspect(i),
      onSkip: () => skip(id),
    };
  };

  const stage = CHECKS[pf.stage];
  const panel =
    stage === "agents" ? (
      <AgentsPanel setup={setup} scanning={pf.states.agents === "checking"} />
    ) : stage === "git" ? (
      <ExhibitParallel />
    ) : (
      <ExhibitCode />
    );

  return (
    <div className="ob-step pf">
      <div className="pf-copy">
        <h2 className="ob-display pf-title ob-reveal" style={{ "--d": ".04s" } as CSSProperties}>
          Pre-flight <em>check.</em>
        </h2>
        <p className="ob-lede ob-reveal" style={{ "--d": ".1s" } as CSSProperties}>
          Three quick checks, mostly automatic. Fletch runs on your machine, with the agents you
          already use.
        </p>
        <ol className="pf-list">
          <AgentsRow setup={setup} {...rowProps("agents")} />
          <GitRow setup={setup} {...rowProps("git")} />
          <GithubRow setup={setup} github={github} {...rowProps("github")} />
        </ol>
        <div className="pf-finish">
          {pf.done && (
            <button type="button" className="ob-cta pf-open" onClick={onFinish}>
              Open Fletch
              <span className="ent">↵</span>
            </button>
          )}
        </div>
      </div>
      <div className="pf-stage">
        <div className="pf-panel" key={stage}>
          {panel}
        </div>
      </div>
    </div>
  );
}
