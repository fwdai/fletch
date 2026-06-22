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

function CheckRow({
  icon,
  name,
  state,
  statusText,
  fix,
  docs,
}: {
  icon: ReactNode;
  name: string;
  state: RowState;
  statusText: string;
  fix?: string;
  docs?: string;
}) {
  const needsFix = state === "bad" || state === "warn";
  return (
    <div className="rdy-check-row">
      <span className="rdy-check-icon">{icon}</span>
      <span className="rdy-check-name">{name}</span>
      <span className={`rdy-dot ${state}`} />
      <span className="rdy-check-status">{statusText}</span>
      {needsFix && (fix || docs) && (
        <div className="rdy-check-fix">
          {fix && <CopyCmd cmd={fix} />}
          {docs && (
            <button type="button" className="rdy-docs" onClick={() => void openExternal(docs)}>
              Setup guide
              <Icon name="external" size={10} />
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function AgentCard({
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
  fix?: string;
  docs?: string;
  hint?: string;
}) {
  const needsFix = state === "bad" || state === "warn";
  return (
    <div className={`rdy-agent-card ${state}`}>
      <div className="rdy-agent-icon">{icon}</div>
      <div className="rdy-agent-name">{name}</div>
      <div className="rdy-agent-foot">
        <span className={`rdy-dot ${state}`} />
        <span className="rdy-agent-status">{statusText}</span>
      </div>
      {needsFix && (fix || docs) && (
        <div className="rdy-agent-fix">
          {fix && <CopyCmd cmd={fix} />}
          {docs && (
            <button type="button" className="rdy-docs" onClick={() => void openExternal(docs)}>
              Install
              <Icon name="external" size={10} />
            </button>
          )}
        </div>
      )}
      {state === "ok" && hint && <div className="rdy-hint rdy-agent-hint">{hint}</div>}
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

  // Only agents with a wired runner are worth checking; antigravity etc. are
  // gated "coming soon" and never spawnable.
  const agents = PROVIDERS.filter((p) => hasAdapter(p.id) && providerFlags[p.id] !== false);
  const detected = agents.filter((p) => !!providerPaths[p.id]).length;

  // git/gh come from local state (null until their probe resolves); agents come
  // from the store's `providersProbed` (= a probe succeeded). In all three, a
  // probe that finished without a result is "couldn't detect" (warn), never a
  // false "not installed".
  const gitState: RowState = git ? (git.installed ? "ok" : "bad") : checking ? "checking" : "warn";
  const ghState: RowState = gh
    ? gh.installed && gh.authenticated
      ? "ok"
      : "warn"
    : checking
      ? "checking"
      : "warn";
  // gh sign-in is universal; install is not — rely on the cross-platform docs
  // link rather than a Homebrew-only `brew install gh`.
  const ghFix = gh && gh.installed && !gh.authenticated ? "gh auth login" : undefined;

  return (
    <div className="readiness">
      <div className="rdy-prereqs">
        <CheckRow
          icon={<Icon name="branch" size={14} />}
          name="Git"
          state={gitState}
          statusText={
            git
              ? git.installed
                ? git.version ?? "Installed"
                : "Not found — required to run any agent"
              : checking
                ? "Checking…"
                : "Couldn't check"
          }
          fix={gitState === "bad" ? "xcode-select --install" : undefined}
          docs="https://git-scm.com/downloads"
        />
        <CheckRow
          icon={<Icon name="github" size={14} />}
          name="GitHub CLI"
          state={ghState}
          statusText={
            gh
              ? !gh.installed
                ? "Not found — needed for clone & PRs"
                : !gh.authenticated
                  ? "Installed · not signed in"
                  : `Signed in${gh.login ? ` as ${gh.login}` : ""}`
              : checking
                ? "Checking…"
                : "Couldn't check"
          }
          fix={ghFix}
          docs={!gh?.installed ? "https://cli.github.com" : undefined}
        />
      </div>

      <div className="rdy-grid">
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
            <AgentCard
              key={p.id}
              icon={<ProviderIcon slug={p.id} short={p.short} hue={p.hue} size={32} />}
              name={p.label}
              state={state}
              statusText={
                state === "checking"
                  ? "Checking…"
                  : state === "warn"
                    ? "Couldn't detect"
                    : path
                      ? providerVersions[p.id] ?? "Installed"
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
      </div>

      <div className="rdy-foot">
        <span className="rdy-count">
          {checking
            ? "Checking…"
            : providersProbed
              ? `${detected} of ${agents.length} agents detected`
              : "Couldn't detect agents"}
        </span>
        <button type="button" className="btn-t outline" onClick={recheck} disabled={checking}>
          <Icon name="refresh" size={12} />
          {checking ? "Checking…" : "Re-check"}
        </button>
      </div>
    </div>
  );
}
