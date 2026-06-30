// Onboarding-finale-only readiness widget. Shows git + gh as two compact
// check rows at the top, then a tight 3-column grid of agent cards below.
// Visually matches the Step 3 exhibit aesthetic — dense rows, small icons.
//
// Intentionally separate from ProviderReadiness (Settings → Providers) so
// that component's flat-list layout is unaffected.

import { open as openExternal } from "@tauri-apps/plugin-shell";
import { useCallback, useEffect, useState } from "react";
import { hasAdapter } from "../../adapters";
import { api, type GhStatus, type ToolStatus } from "../../api";
import { PROVIDER_DETAIL } from "../../data/providerDetail";
import { PROVIDERS } from "../../data/providers";
import { useAppStore } from "../../store";
import { Icon } from "../Icon";
import { ProviderIcon } from "../ProviderIcon";
import { Button } from "../ui/Button";

type S = "ok" | "warn" | "bad" | "checking";

export function OnboardingReadiness() {
  const providerVersions = useAppStore((s) => s.providerVersions);
  const providerPaths = useAppStore((s) => s.providerPaths);
  const providersProbed = useAppStore((s) => s.providersProbed);
  const refresh = useAppStore((s) => s.refreshProviderVersions);
  const providerFlags = useAppStore((s) => s.providerFlags);

  const [git, setGit] = useState<ToolStatus | null>(null);
  const [gh, setGh] = useState<GhStatus | null>(null);
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

  const agents = PROVIDERS.filter((p) => hasAdapter(p.id) && providerFlags[p.id] !== false);
  const detected = agents.filter((p) => !!providerPaths[p.id]).length;

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
    <div className="ob-rdy">
      {/* Prereq rows: git + gh */}
      <div className="ob-rdy-prereqs">
        <CheckRow
          icon={<Icon name="branch" size={13} />}
          name="Git"
          state={gitState}
          status={
            git
              ? git.installed
                ? (git.version ?? "installed")
                : "not found"
              : checking
                ? "checking…"
                : "couldn't check"
          }
          fix={gitState === "bad" ? "xcode-select --install" : undefined}
          docs="https://git-scm.com/downloads"
        />
        <CheckRow
          icon={<Icon name="github" size={13} />}
          name="GitHub CLI"
          state={ghState}
          status={
            gh
              ? !gh.installed
                ? "not found"
                : !gh.authenticated
                  ? "not signed in"
                  : `signed in${gh.login ? ` · ${gh.login}` : ""}`
              : checking
                ? "checking…"
                : "couldn't check"
          }
          fix={ghFix}
          docs={!gh?.installed ? "https://cli.github.com" : undefined}
        />
      </div>

      {/* Agent grid */}
      <div className="ob-rdy-grid">
        {agents.map((p) => {
          const d = PROVIDER_DETAIL[p.id];
          const path = providerPaths[p.id];
          const state: S = checking ? "checking" : providersProbed ? (path ? "ok" : "bad") : "warn";
          return (
            <AgentTile
              key={p.id}
              icon={<ProviderIcon slug={p.id} short={p.short} hue={p.hue} size={26} />}
              name={p.label}
              state={state}
              status={
                state === "checking"
                  ? "checking…"
                  : state === "warn"
                    ? "couldn't detect"
                    : path
                      ? (providerVersions[p.id] ?? "installed")
                      : "not installed"
              }
              fix={state === "bad" ? d.install : undefined}
              docs={d.docs}
            />
          );
        })}
      </div>

      <div className="ob-rdy-foot">
        <span className="ob-rdy-count text-sm">
          {checking
            ? "checking…"
            : providersProbed
              ? `${detected} of ${agents.length} agents detected`
              : "couldn't detect agents"}
        </span>
        <Button variant="outline" onClick={recheck} disabled={checking}>
          <Icon name="refresh" size={11} />
          {checking ? "checking…" : "re-check"}
        </Button>
      </div>
    </div>
  );
}

function CheckRow({
  icon,
  name,
  state,
  status,
  fix,
  docs,
}: {
  icon: React.ReactNode;
  name: string;
  state: S;
  status: string;
  fix?: string;
  docs?: string;
}) {
  const needsFix = state === "bad" || state === "warn";
  return (
    <div className="ob-rdy-check">
      <span className="ob-rdy-check-icon">{icon}</span>
      <span className="ob-rdy-check-name">{name}</span>
      <span className={`ob-rdy-dot ${state}`} />
      <span className="ob-rdy-check-status">{status}</span>
      {needsFix && (fix || docs) && (
        <div className="ob-rdy-check-fix">
          {fix && <CopyCmd cmd={fix} />}
          {docs && (
            <button
              type="button"
              className="rdy-docs iflex-center"
              onClick={() => void openExternal(docs)}
            >
              guide <Icon name="external" size={9} />
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function AgentTile({
  icon,
  name,
  state,
  status,
  fix,
  docs,
}: {
  icon: React.ReactNode;
  name: string;
  state: S;
  status: string;
  fix?: string;
  docs?: string;
}) {
  const needsFix = state === "bad" || state === "warn";
  return (
    <div className={`ob-rdy-tile ${state}`}>
      <span className="ob-rdy-tile-icon">{icon}</span>
      <span className="ob-rdy-tile-name">{name}</span>
      <span className="ob-rdy-tile-foot">
        <span className={`ob-rdy-dot ${state}`} />
        <span className="ob-rdy-tile-status">{status}</span>
      </span>
      {needsFix && (fix || docs) && (
        <div className="ob-rdy-tile-fix">
          {fix && <CopyCmd cmd={fix} />}
          {docs && (
            <button
              type="button"
              className="rdy-docs iflex-center"
              onClick={() => void openExternal(docs)}
            >
              install <Icon name="external" size={9} />
            </button>
          )}
        </div>
      )}
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
      <Icon name={copied ? "check" : "copy"} size={10} />
    </button>
  );
}
