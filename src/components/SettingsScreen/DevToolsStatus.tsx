import { type ReactNode, useCallback, useEffect, useState } from "react";
import { api, type GhStatus, type ToolStatus } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { CopyCmd } from "@/components/ui/CopyCmd";
import { DocsLink } from "@/components/ui/DocsLink";
import { IS_MAC } from "@/util/platform";
import { useGitInstall } from "@/util/useGitInstall";

type S = "ok" | "warn" | "bad" | "checking";

export function DevToolsStatus() {
  const [git, setGit] = useState<ToolStatus | null>(null);
  const [gh, setGh] = useState<GhStatus | null>(null);
  const [checking, setChecking] = useState(true);

  const recheck = useCallback(() => {
    setChecking(true);
    void Promise.allSettled([api.checkCli("git").then(setGit), api.ghStatus().then(setGh)]).finally(
      () => setChecking(false),
    );
  }, []);

  useEffect(() => {
    recheck();
  }, [recheck]);

  const { gitDownloading, gitInstallError, installingGit, installGit } = useGitInstall(
    git,
    recheck,
  );

  const gitState: S = gitDownloading
    ? "checking"
    : git
      ? git.installed
        ? "ok"
        : "bad"
      : checking
        ? "checking"
        : "warn";
  const ghState: S = gh ? (gh.authenticated ? "ok" : "warn") : checking ? "checking" : "warn";

  return (
    <div className="devtools">
      <ToolRow
        icon={<Icon name="branch" size={15} />}
        name="Git"
        state={gitState}
        statusText={
          gitDownloading
            ? "Downloading portable Git…"
            : git
              ? git.installed
                ? git.source === "portable"
                  ? `${git.version ?? "Installed"}, bundled with Fletch`
                  : (git.version ?? "Installed")
                : gitInstallError
                  ? `Install failed: ${gitInstallError}`
                  : "Not found. Required to run any agent."
              : checking
                ? "Checking…"
                : "Couldn't check"
        }
        action={
          gitState === "bad" ? (
            <Button variant="outline" onClick={installGit} disabled={installingGit}>
              {installingGit ? "Installing…" : "Install Git"}
            </Button>
          ) : undefined
        }
        fix={gitState === "bad" && IS_MAC ? "xcode-select --install" : undefined}
        docs="https://git-scm.com/downloads"
      />
      <ToolRow
        icon={<Icon name="github" size={15} />}
        name="GitHub"
        state={ghState}
        statusText={
          gh
            ? gh.authenticated
              ? `Connected${gh.login ? ` as ${gh.login}` : ""}`
              : "Not connected. Sign in with GitHub (Account tab) for clone & PRs."
            : checking
              ? "Checking…"
              : "Couldn't check"
        }
      />
      <div className="devtools-foot flex-center">
        <Button variant="outline" onClick={recheck} disabled={checking}>
          <Icon name="refresh" size={12} />
          {checking ? "Checking…" : "Re-check"}
        </Button>
      </div>
    </div>
  );
}

function ToolRow({
  icon,
  name,
  state,
  statusText,
  action,
  fix,
  docs,
}: {
  icon: ReactNode;
  name: string;
  state: S;
  statusText: string;
  /** In-app remediation (e.g. the Install Git button), shown before the
   *  copy-paste fix. */
  action?: ReactNode;
  fix?: string;
  docs?: string;
}) {
  const needsFix = state === "bad" || state === "warn";
  return (
    <div className="devtools-row">
      <span className="devtools-icon">{icon}</span>
      <div className="devtools-main">
        <div className="devtools-line flex-center">
          <span className="devtools-name">{name}</span>
          <span className={`devtools-dot ${state}`} />
          <span className="devtools-status">{statusText}</span>
        </div>
        {needsFix && (action || fix || docs) && (
          <div className="devtools-fix flex-center">
            {action}
            {fix && <CopyCmd cmd={fix} />}
            {docs && <DocsLink url={docs} />}
          </div>
        )}
      </div>
    </div>
  );
}
