// Row 02 · Git. Every checkout runs on it, so the app already resolves git at
// launch — system git, or a bundled copy it downloads itself when there is
// none. By the time this row is reached the usual answer is a version number;
// a download still in flight shows its progress, and only a failed one asks
// anything of the user.

import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { DocsLink } from "@/components/ui/DocsLink";
import { IS_MAC } from "@/util/platform";
import { CopyCmd } from "../CopyCmd";
import type { OnboardingSetup } from "../useSetup";
import { CheckRow } from "./CheckRow";
import type { CheckState } from "./usePreflight";

const GIT_DOCS = "https://git-scm.com/downloads";

export function GitRow({
  setup,
  state,
  active,
  onInspect,
  onSkip,
}: {
  setup: OnboardingSetup;
  state: CheckState;
  active: boolean;
  onInspect: () => void;
  onSkip: () => void;
}) {
  const { git, gitDist, gitDownloading, gitInstallError, installingGit, installGit } = setup;
  const settingUp = gitDownloading || installingGit;
  const pct =
    gitDist.received && gitDist.total ? Math.round((gitDist.received / gitDist.total) * 100) : null;

  let detail: string;
  switch (state) {
    case "pending":
      detail = "Every task gets its own checkout";
      break;
    case "checking":
      detail = settingUp
        ? `Setting up a bundled copy${pct !== null ? ` · ${pct}%` : "…"}`
        : "Checking for Git…";
      break;
    case "ok":
      detail = `Git ${git?.version ?? ""} · ${
        git?.source === "portable" ? "bundled with Fletch" : "found on this machine"
      }`.replace("  ", " ");
      break;
    case "skipped":
      detail = "Skipped · Fletch needs Git to run agents";
      break;
    default:
      detail = gitInstallError ? "Couldn't set up Git" : "Git not found";
  }

  return (
    <CheckRow
      index={1}
      title="Git"
      detail={detail}
      state={state}
      active={active}
      onInspect={onInspect}
    >
      {state === "checking" && settingUp && (
        <div className={`ob-progress ${pct === null ? "indet" : ""}`}>
          <i style={pct !== null ? { width: `${pct}%` } : undefined} />
        </div>
      )}
      {state === "attention" && (
        <>
          {gitInstallError ? (
            <p className="pf-err">{gitInstallError}</p>
          ) : (
            <p className="pf-note">Fletch can install its own copy — nothing to manage.</p>
          )}
          <div className="pf-actions">
            <Button variant="primary" onClick={installGit}>
              <Icon name="download" size={12} />
              {gitInstallError ? "Try again" : "Install Git"}
            </Button>
            {IS_MAC && <CopyCmd cmd="xcode-select --install" />}
            <DocsLink url={GIT_DOCS} />
          </div>
          <button type="button" className="ob-skiplink" onClick={onSkip}>
            I'll install it myself
          </button>
        </>
      )}
    </CheckRow>
  );
}
