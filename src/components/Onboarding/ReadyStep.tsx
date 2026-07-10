// Step 04 · Ready. The handoff: a summary of the three requirements and the
// Enter CTA, with the control-room exhibit as the send-off visual. Reachable
// with gaps via Skip — unmet rows show amber with a pointer to Settings, and
// Enter stays enabled (never trap the user; the app degrades gracefully).

import type { CSSProperties, ReactNode } from "react";
import { Icon } from "@/components/Icon";
import { ExhibitRoom } from "./exhibits";
import { SetupStep } from "./SetupBits";
import type { OnboardingSetup } from "./useSetup";

function SumRow({ name, ok, status }: { name: string; ok: boolean; status: ReactNode }) {
  return (
    <div className="ob-sum-row">
      <span className={`ob-rdy-dot ${ok ? "ok" : "warn"}`} />
      <span className="nm">{name}</span>
      <span className="st">{status}</span>
    </div>
  );
}

export function ReadyStep({ setup, onEnter }: { setup: OnboardingSetup; onEnter: () => void }) {
  const { git, gitReady, gh, ghConnected, detected, hasAgent } = setup;
  const allSet = gitReady && hasAgent;

  return (
    <SetupStep
      num="04"
      eyebrow="One quiet control room"
      title={
        allSet ? (
          <>
            You're <em>all set.</em>
          </>
        ) : (
          <>
            Almost <em>there.</em>
          </>
        )
      }
      lede={
        allSet ? (
          <>
            Home shows what's running, what's waiting, and what needs you — across every project.{" "}
            <b>Add your first repo and put an agent to work.</b>
          </>
        ) : (
          <>
            A couple of things are still pending — you can finish them anytime in{" "}
            <b>Settings › Providers.</b>
          </>
        )
      }
      points={[]}
      exhibit={<ExhibitRoom />}
    >
      <div className="ob-sum ob-reveal" style={{ "--d": ".34s" } as CSSProperties}>
        <SumRow
          name="Git"
          ok={gitReady}
          status={
            gitReady
              ? `${git?.version ?? "installed"}${git?.source === "portable" ? " · bundled" : ""}`
              : "not installed — required to run agents"
          }
        />
        <SumRow
          name="GitHub"
          ok={ghConnected}
          status={
            ghConnected
              ? `connected${gh?.login ? ` · ${gh.login}` : ""}`
              : "skipped · optional — connect in Settings › Account"
          }
        />
        <SumRow
          name="Agents"
          ok={hasAgent}
          status={hasAgent ? `${detected} detected` : "none installed yet"}
        />
      </div>
      <div className="ob-ready-cta ob-reveal" style={{ "--d": ".46s" } as CSSProperties}>
        <button type="button" className="ob-cta" onClick={onEnter}>
          Enter Fletch
          <Icon name="arrowR" />
        </button>
        <p className="ob-fineprint text-sm">
          Fletch shares anonymous usage data to improve the app. Turn it off anytime in Settings ›
          General.
        </p>
      </div>
    </SetupStep>
  );
}
