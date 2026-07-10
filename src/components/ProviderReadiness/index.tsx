// First-run / settings readiness check. Reusable surface that shows whether
// the tools needed to actually run agents are present on this machine — git
// (required), each wired agent CLI, and the GitHub connection (optional, for
// clone/PRs) — with copy-paste fixes. Used in the onboarding finale and in
// Settings → Providers.
//
// We detect *binary presence* (and the GitHub API connection, which we can
// read), not agent auth (varies per CLI), so installed rows still nudge the
// user to sign in.

import { open as openExternal } from "@tauri-apps/plugin-shell";
import { type ReactNode, useCallback, useEffect, useState } from "react";
import { api, type GhStatus, type ToolStatus } from "@/api";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Button } from "@/components/ui/Button";
import { PROVIDER_DETAIL } from "@/data/providerDetail";
import { PROVIDERS } from "@/data/providers";
import { useAppStore } from "@/store";
import { IS_MAC } from "@/util/platform";
import { useGitDist } from "@/util/useGitDist";

type RowState = "ok" | "warn" | "bad" | "checking";

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

function Row({
  icon,
  name,
  state,
  statusText,
  action,
  fix,
  docs,
  hint,
}: {
  icon: ReactNode;
  name: string;
  state: RowState;
  statusText: string;
  /** In-app remediation (e.g. the Install Git button), shown before the
   *  copy-paste fix. */
  action?: ReactNode;
  /** Copy-paste command that resolves the issue (install or sign-in). */
  fix?: string;
  docs?: string;
  /** Shown when installed — e.g. a sign-in nudge we can't verify. */
  hint?: string;
}) {
  const needsFix = state === "bad" || state === "warn";
  return (
    <div className="rdy-row">
      <span className="rdy-icon">{icon}</span>
      <div className="rdy-main">
        <div className="rdy-line flex-center">
          <span className="rdy-name text-base">{name}</span>
          <span className={`rdy-dot ${state}`} />
          <span className="rdy-status text-sm">{statusText}</span>
        </div>
        {needsFix && (action || fix || docs) && (
          <div className="rdy-fix flex-center">
            {action}
            {fix && <CopyCmd cmd={fix} />}
            {docs && (
              <button
                type="button"
                className="rdy-docs iflex-center text-sm"
                onClick={() => void openExternal(docs)}
              >
                Setup guide
                <Icon name="external" size={10} />
              </button>
            )}
          </div>
        )}
        {state === "ok" && hint && <div className="rdy-hint text-sm">{hint}</div>}
      </div>
    </div>
  );
}

export function ProviderReadiness() {
  const providerVersions = useAppStore((s) => s.providerVersions);
  const providerPaths = useAppStore((s) => s.providerPaths);
  const providersProbed = useAppStore((s) => s.providersProbed);
  const refresh = useAppStore((s) => s.refreshProviderVersions);
  const providerFlags = useAppStore((s) => s.providerFlags);

  const [git, setGit] = useState<ToolStatus | null>(null);
  const [gh, setGh] = useState<GhStatus | null>(null);
  // Start in the checking state — we always probe on mount, so this avoids a
  // flash of "couldn't detect" before the effect runs.
  const [checking, setChecking] = useState(true);

  const recheck = useCallback(() => {
    setChecking(true);
    void Promise.allSettled([
      refresh(),
      api.checkCli("git").then(setGit),
      api.ghStatus().then(setGh),
    ]).finally(() => setChecking(false));
  }, [refresh]);

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

  // Skip agents the user has toggled off; the rest are all runnable.
  const agents = PROVIDERS.filter((p) => providerFlags[p.id] !== false);
  const detected = agents.filter((p) => !!providerPaths[p.id]).length;

  // git/gh come from local state (null until their probe resolves); agents come
  // from the store's `providersProbed` (= a probe succeeded). In all three, a
  // probe that finished without a result is "couldn't detect" (warn), never a
  // false "not installed".
  const gitState: RowState = gitDownloading
    ? "checking"
    : git
      ? git.installed
        ? "ok"
        : "bad"
      : checking
        ? "checking"
        : "warn";
  const ghState: RowState = gh
    ? gh.authenticated
      ? "ok"
      : "warn"
    : checking
      ? "checking"
      : "warn";

  return (
    <div className="readiness">
      <Row
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

      {agents.map((p) => {
        const d = PROVIDER_DETAIL[p.id];
        const path = providerPaths[p.id];
        const state: RowState = checking
          ? "checking"
          : providersProbed
            ? path
              ? "ok"
              : "bad"
            : "warn";
        return (
          <Row
            key={p.id}
            icon={<ProviderIcon slug={p.id} short={p.short} hue={p.hue} size={18} />}
            name={p.label}
            state={state}
            statusText={
              state === "checking"
                ? "Checking…"
                : state === "warn"
                  ? "Couldn't detect"
                  : path
                    ? (providerVersions[p.id] ?? "Installed")
                    : "Not installed"
            }
            // Only offer the install command when we know it's missing, not
            // when detection itself failed.
            fix={state === "bad" ? d.install : undefined}
            docs={d.docs}
            hint={d.signIn}
          />
        );
      })}

      <Row
        icon={<Icon name="github" size={15} />}
        name="GitHub · optional"
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
        <span className="rdy-count text-sm">
          {checking
            ? "Checking…"
            : providersProbed
              ? `${detected} of ${agents.length} agents detected`
              : "Couldn't detect agents"}
        </span>
        <Button variant="outline" onClick={recheck} disabled={checking}>
          <Icon name="refresh" size={12} />
          {checking ? "Checking…" : "Re-check"}
        </Button>
      </div>
    </div>
  );
}
