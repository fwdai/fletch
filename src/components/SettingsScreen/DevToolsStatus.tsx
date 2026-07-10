import { open as openExternal } from "@tauri-apps/plugin-shell";
import { type ReactNode, useCallback, useEffect, useState } from "react";
import { api, type GhStatus, type ToolStatus } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { IS_MAC } from "@/util/platform";
import { useGitDist } from "@/util/useGitDist";

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

  // While the app is downloading its portable git (no usable system git),
  // show that instead of a false "not found"; re-check once it settles.
  const gitDist = useGitDist(recheck);
  const gitDownloading = !git?.installed && gitDist.phase === "downloading";
  // The startup bootstrap's failure reason, shown until a retry succeeds.
  const gitInstallError = !git?.installed && gitDist.phase === "failed" ? gitDist.error : undefined;

  const [installingGit, setInstallingGit] = useState(false);
  const installGit = useCallback(() => {
    setInstallingGit(true);
    // Progress + failure reason arrive via git-dist:state; the final recheck
    // covers the case where the bootstrap already settled before mount (no
    // further events) yet the retry succeeded.
    void api
      .gitDistInstall()
      .catch(() => {})
      .finally(() => {
        setInstallingGit(false);
        recheck();
      });
  }, [recheck]);

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
    <div className="readiness">
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
                  ? `${git.version ?? "Installed"} — bundled with Fletch`
                  : (git.version ?? "Installed")
                : gitInstallError
                  ? `Install failed — ${gitInstallError}`
                  : "Not found — required to run any agent"
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
              : "Not connected — sign in with GitHub (Account tab) for clone & PRs"
            : checking
              ? "Checking…"
              : "Couldn't check"
        }
      />
      <div className="rdy-foot flex-center">
        <span className="rdy-count" />
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
    <div className="rdy-row">
      <span className="rdy-icon">{icon}</span>
      <div className="rdy-main">
        <div className="rdy-line flex-center">
          <span className="rdy-name">{name}</span>
          <span className={`rdy-dot ${state}`} />
          <span className="rdy-status">{statusText}</span>
        </div>
        {needsFix && (action || fix || docs) && (
          <div className="rdy-fix flex-center">
            {action}
            {fix && <CopyCmd cmd={fix} />}
            {docs && (
              <button
                type="button"
                className="rdy-docs iflex-center"
                onClick={() => void openExternal(docs)}
              >
                Setup guide
                <Icon name="external" size={10} />
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function CopyCmd({ cmd }: { cmd: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className="rdy-cmd iflex-center"
      title="Copy command"
      onClick={() => {
        void navigator.clipboard.writeText(cmd);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      }}
    >
      <code>{cmd}</code>
      <Icon name={copied ? "check" : "copy"} size={11} />
    </button>
  );
}
