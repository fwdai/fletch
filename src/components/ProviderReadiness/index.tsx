// First-run / settings readiness check. Reusable surface that shows whether
// the tools needed to actually run agents are present on this machine — git
// (required), each wired agent CLI, and the GitHub CLI (optional, for
// clone/PRs) — with copy-paste fixes. Used in the onboarding finale and in
// Settings → Providers.
//
// We detect *binary presence* (and gh auth, which we can read), not agent
// auth (varies per CLI), so installed rows still nudge the user to sign in.

import { open as openExternal } from "@tauri-apps/plugin-shell";
import { type ReactNode, useCallback, useEffect, useState } from "react";
import { hasAdapter } from "@/adapters";
import { api, type GhStatus, type ToolStatus } from "@/api";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Button } from "@/components/ui/Button";
import { PROVIDER_DETAIL } from "@/data/providerDetail";
import { PROVIDERS } from "@/data/providers";
import { useAppStore } from "@/store";

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
        <div className="rdy-line flex-center">
          <span className="rdy-name text-base">{name}</span>
          <span className={`rdy-dot ${state}`} />
          <span className="rdy-status text-sm">{statusText}</span>
        </div>
        {needsFix && (fix || docs) && (
          <div className="rdy-fix flex-center">
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
  const ghFix = gh?.installed && !gh.authenticated ? "gh auth login" : undefined;

  return (
    <div className="readiness">
      <Row
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
        name="GitHub CLI · optional"
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
