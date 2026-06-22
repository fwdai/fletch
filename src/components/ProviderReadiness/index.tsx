// First-run / settings readiness check. Reusable surface that shows whether
// the tools needed to actually run agents are present on this machine — git
// (required), each wired agent CLI, and the GitHub CLI (optional, for
// clone/PRs) — with copy-paste fixes. Used in the onboarding finale and in
// Settings → Providers.
//
// We detect *binary presence* (and gh auth, which we can read), not agent
// auth (varies per CLI), so installed rows still nudge the user to sign in.

import { useCallback, useEffect, useState, type ReactNode } from "react";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { useAppStore } from "../../store";
import { api, type GhStatus, type ToolStatus } from "../../api";
import { PROVIDERS } from "../../data/providers";
import { PROVIDER_DETAIL } from "../../data/providerDetail";
import { hasAdapter } from "../../adapters";
import { ProviderIcon } from "../ProviderIcon";
import { Icon } from "../Icon";

type RowState = "ok" | "warn" | "bad" | "checking";

function CopyCmd({ cmd }: { cmd: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className="rdy-cmd"
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
  fix,
  docs,
  hint,
}: {
  icon: ReactNode;
  name: string;
  state: RowState;
  statusText: string;
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
        <div className="rdy-line">
          <span className="rdy-name">{name}</span>
          <span className={`rdy-dot ${state}`} />
          <span className="rdy-status">{statusText}</span>
        </div>
        {needsFix && (fix || docs) && (
          <div className="rdy-fix">
            {fix && <CopyCmd cmd={fix} />}
            {docs && (
              <button type="button" className="rdy-docs" onClick={() => void openExternal(docs)}>
                Setup guide
                <Icon name="external" size={10} />
              </button>
            )}
          </div>
        )}
        {state === "ok" && hint && <div className="rdy-hint">{hint}</div>}
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
  const [checking, setChecking] = useState(false);

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

  // Only agents with a wired runner are worth checking; antigravity etc. are
  // gated "coming soon" and never spawnable.
  const agents = PROVIDERS.filter((p) => hasAdapter(p.id) && providerFlags[p.id] !== false);
  const detected = agents.filter((p) => !!providerPaths[p.id]).length;

  const gitState: RowState = git ? (git.installed ? "ok" : "bad") : "checking";
  const ghState: RowState = gh ? (gh.installed && gh.authenticated ? "ok" : "warn") : "checking";
  const ghFix = !gh
    ? undefined
    : !gh.installed
      ? "brew install gh"
      : !gh.authenticated
        ? "gh auth login"
        : undefined;

  return (
    <div className="readiness">
      <Row
        icon={<Icon name="branch" size={15} />}
        name="Git"
        state={gitState}
        statusText={
          gitState === "checking"
            ? "Checking…"
            : git?.installed
              ? git.version ?? "Installed"
              : "Not found — required to run any agent"
        }
        fix="xcode-select --install"
        docs="https://git-scm.com/downloads"
      />

      {agents.map((p) => {
        const d = PROVIDER_DETAIL[p.id];
        const path = providerPaths[p.id];
        const state: RowState = !providersProbed ? "checking" : path ? "ok" : "bad";
        return (
          <Row
            key={p.id}
            icon={<ProviderIcon slug={p.id} short={p.short} hue={p.hue} size={18} />}
            name={p.label}
            state={state}
            statusText={
              state === "checking"
                ? "Checking…"
                : path
                  ? providerVersions[p.id] ?? "Installed"
                  : "Not installed"
            }
            fix={d.install}
            docs={d.docs}
            hint={d.signIn}
          />
        );
      })}

      <Row
        icon={<Icon name="github" size={15} />}
        name="GitHub CLI · optional"
        state={ghState}
        statusText={
          !gh
            ? "Checking…"
            : !gh.installed
              ? "Not found — needed for clone & PRs"
              : !gh.authenticated
                ? "Installed — not signed in"
                : `Signed in${gh.login ? ` as ${gh.login}` : ""}`
        }
        fix={ghFix}
        docs={gh && !gh.installed ? "https://cli.github.com" : undefined}
      />

      <div className="rdy-foot">
        <span className="rdy-count">
          {detected} of {agents.length} agents detected
        </span>
        <button type="button" className="btn-t outline" onClick={recheck} disabled={checking}>
          <Icon name="refresh" size={12} />
          {checking ? "Checking…" : "Re-check"}
        </button>
      </div>
    </div>
  );
}
