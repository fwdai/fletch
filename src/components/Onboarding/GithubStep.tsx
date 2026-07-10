// Step 02 · GitHub. Auto-satisfied when the user signed in with GitHub on the
// welcome step (that flow persists a token); a Google sign-in is identity-only,
// so this is where those users grant repo access. Recommended but skippable —
// GitLab / Bitbucket / SSH / local-only users can pass, and the Git panel's
// existing connect gate catches them later at the moment of need.

import type { CSSProperties } from "react";
import { Icon } from "@/components/Icon";
import { useGithubConnect } from "@/util/useGithubConnect";
import { ExhibitCode } from "./exhibits";
import { SetupStep } from "./SetupBits";
import type { OnboardingSetup } from "./useSetup";

export function GithubStep({ setup, onSkip }: { setup: OnboardingSetup; onSkip: () => void }) {
  const { ghConnected, gh, recheck } = setup;
  // Own device-flow instance: on success re-run the shared checks so the
  // footer's Continue enables without leaving the step.
  const { connect, cancel, device, error, busy } = useGithubConnect(recheck);

  let card: React.ReactNode;
  if (ghConnected) {
    card = (
      <div className="ob-setup-line">
        <span className="ob-rdy-dot ok" />
        <span>
          <b>Connected{gh?.login ? ` as ${gh.login}` : ""}</b> — pull requests, clone, and push are
          ready
        </span>
      </div>
    );
  } else if (error) {
    card = (
      <>
        <div className="ob-setup-line">
          <span className="ob-rdy-dot bad" />
          <span className="ob-setup-err">{error}</span>
        </div>
        <div className="ob-setup-actions">
          <button type="button" className="ob-cta sm" onClick={() => void connect("github")}>
            Try again
          </button>
          <button type="button" className="ob-cancel text-sm" onClick={cancel}>
            Cancel
          </button>
        </div>
      </>
    );
  } else if (device) {
    card = (
      <>
        <div className="ob-setup-line">
          <span className="ob-spinner" />
          <span>Finish in your browser, then enter this code:</span>
        </div>
        <div className="ob-setup-code">{device.userCode}</div>
        <div className="ob-setup-actions">
          <span className="ob-setup-sub">{device.verificationUri}</span>
          <button type="button" className="ob-cancel text-sm" onClick={cancel}>
            Cancel
          </button>
        </div>
      </>
    );
  } else if (busy) {
    card = (
      <div className="ob-setup-line">
        <span className="ob-spinner" />
        <span>Starting GitHub sign-in…</span>
      </div>
    );
  } else {
    card = (
      <div className="ob-setup-actions">
        <button type="button" className="ob-cta sm" onClick={() => void connect("github")}>
          <Icon name="github" size={15} />
          Connect GitHub
        </button>
      </div>
    );
  }

  return (
    <SetupStep
      num="02"
      eyebrow="From checkout to shipped"
      title={
        <>
          Ship straight to a <em>pull request.</em>
        </>
      }
      lede={
        <>
          Connect GitHub so Fletch can open pull requests, clone private repos, and push branches
          for you. <b>One connection, the whole loop.</b>
        </>
      }
      points={[
        { icon: "pr", head: "PRs from any checkout.", body: "Review, refine, and ship in place." },
        { icon: "branch", head: "Clone & push.", body: "Private repos included, over HTTPS." },
        { icon: "commit", head: "Live PR status.", body: "Checks and reviews, at a glance." },
      ]}
      exhibit={<ExhibitCode />}
    >
      <div className="ob-setup-card ob-reveal" style={{ "--d": ".5s" } as CSSProperties}>
        {card}
      </div>
      {!ghConnected && !busy && !device && (
        <button
          type="button"
          className="ob-skiplink ob-reveal"
          style={{ "--d": ".6s" } as CSSProperties}
          onClick={onSkip}
        >
          I use GitLab, Bitbucket, or local repos — skip for now
        </button>
      )}
    </SetupStep>
  );
}
