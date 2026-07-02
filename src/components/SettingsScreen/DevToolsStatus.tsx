import { open as openExternal } from "@tauri-apps/plugin-shell";
import { type ReactNode, useCallback, useEffect, useState } from "react";
import { api, type GhStatus, type ToolStatus } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";

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

  const gitState: S = git ? (git.installed ? "ok" : "bad") : checking ? "checking" : "warn";
  const ghState: S = gh
    ? gh.installed && gh.authenticated
      ? "ok"
      : "warn"
    : checking
      ? "checking"
      : "warn";
  const ghFix = gh?.installed && !gh.authenticated ? "gh auth login" : undefined;

  return (
    <div className="readiness">
      <ToolRow
        icon={<Icon name="branch" size={15} />}
        name="Git"
        state={gitState}
        statusText={
          git
            ? git.installed
              ? (git.version ?? "Installed")
              : "Not found — required to run any agent"
            : checking
              ? "Checking…"
              : "Couldn't check"
        }
        fix={gitState === "bad" ? "xcode-select --install" : undefined}
        docs="https://git-scm.com/downloads"
      />
      <ToolRow
        icon={<Icon name="github" size={15} />}
        name="GitHub CLI"
        state={ghState}
        statusText={
          gh
            ? !gh.installed
              ? "Not found — needed for clone & PRs"
              : !gh.authenticated
                ? "Installed — not signed in"
                : `Signed in${gh.login ? ` as ${gh.login}` : ""}`
            : checking
              ? "Checking…"
              : "Couldn't check"
        }
        fix={ghFix}
        docs={!gh?.installed ? "https://cli.github.com" : undefined}
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
  fix,
  docs,
}: {
  icon: ReactNode;
  name: string;
  state: S;
  statusText: string;
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
        {needsFix && (fix || docs) && (
          <div className="rdy-fix flex-center">
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
