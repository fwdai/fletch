// Step 01 · Git. Functional: shows the true resolution state (system git,
// bundled portable git, downloading, or missing) and can install the portable
// dist in-app. The startup bootstrap usually finishes while the user is still
// signing in, so the common case here is an instant green check — the step
// gates Continue only when git genuinely isn't usable yet.

import type { CSSProperties } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { IS_MAC } from "@/util/platform";
import { ExhibitParallel } from "./exhibits";
import { CopyCmd, DocsLink, SetupStep } from "./SetupBits";
import type { OnboardingSetup } from "./useSetup";

const GIT_DOCS = "https://git-scm.com/downloads";

export function GitStep({ setup }: { setup: OnboardingSetup }) {
  const { git, gitDist, gitReady, gitDownloading, gitInstallError, installingGit, installGit } =
    setup;

  const pct =
    gitDist.received && gitDist.total ? Math.round((gitDist.received / gitDist.total) * 100) : null;

  let line: React.ReactNode;
  if (gitReady) {
    line = (
      <>
        <span className="ob-rdy-dot ok" />
        <span>
          <b>Git {git?.version ?? "detected"}</b>
          {git?.source === "portable"
            ? " — bundled with Fletch, nothing to manage"
            : " — found on this machine"}
        </span>
      </>
    );
  } else if (gitDownloading || installingGit) {
    line = (
      <>
        <span className="ob-spinner" />
        <span>
          <b>Setting up Git</b>
          {pct !== null ? ` — ${pct}%` : "…"}
        </span>
      </>
    );
  } else if (git || !setup.checking) {
    line = (
      <>
        <span className="ob-rdy-dot bad" />
        <span>
          <b>Git not found</b>
          {gitInstallError ? ` — ${gitInstallError}` : " — Fletch can install its own copy"}
        </span>
      </>
    );
  } else {
    line = (
      <>
        <span className="ob-spinner" />
        <span>Checking for Git…</span>
      </>
    );
  }

  const showInstall =
    !gitReady && !gitDownloading && !installingGit && (git !== null || !setup.checking);

  return (
    <SetupStep
      num="01"
      eyebrow="Parallel by design"
      title={
        <>
          Every task gets its <em>own checkout.</em>
        </>
      }
      lede={
        <>
          Fletch spins up as many agents as the work demands — each on an isolated branch, so
          nothing collides. <b>All of it runs on Git.</b>
        </>
      }
      points={[
        {
          icon: "branch",
          head: "Isolated branches.",
          body: "No stepping on each other's changes.",
        },
        { icon: "layers", head: "Run in parallel.", body: "Five tasks, five agents, one glance." },
        { icon: "map", head: "Named by landmark.", body: "Each checkout is easy to find again." },
      ]}
      exhibit={<ExhibitParallel />}
    >
      <div className="ob-setup-card ob-reveal" style={{ "--d": ".5s" } as CSSProperties}>
        <div className="ob-setup-line">{line}</div>
        {(gitDownloading || installingGit) && (
          <div className={`ob-progress ${pct === null ? "indet" : ""}`}>
            <i style={pct !== null ? { width: `${pct}%` } : undefined} />
          </div>
        )}
        {showInstall && (
          <div className="ob-setup-actions">
            <Button variant="outline" onClick={installGit}>
              <Icon name="download" size={12} />
              Install Git
            </Button>
            {IS_MAC && <CopyCmd cmd="xcode-select --install" />}
            <DocsLink url={GIT_DOCS} />
          </div>
        )}
      </div>
    </SetupStep>
  );
}
