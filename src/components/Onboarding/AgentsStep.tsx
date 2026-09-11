// Step 03 · Agents. The one hard gate: Fletch is unusable without at least
// one agent CLI, so Continue stays disabled until a probe finds one. Each
// missing agent offers a one-click install (the backend runs its official
// native installer — see src-tauri/src/agent_install.rs) with the exact
// command shown for transparency; while this step is on screen the shared
// setup hook re-probes every few seconds, so an install finishing — ours or
// one the user ran in their own terminal — lights the tile up by itself.
//
// The install state machine itself lives in the store (store/agentInstall.ts),
// shared with Settings › Providers: this step only renders it.

import type { CSSProperties } from "react";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { DocsLink } from "@/components/ui/DocsLink";
import { installCommand, PROVIDER_DETAIL } from "@/data/providerDetail";
import { useAppStore } from "@/store";
import { ExBar } from "./exhibits";
import { CopyCmd, SetupStep } from "./SetupBits";
import type { OnboardingSetup } from "./useSetup";

export function AgentsStep({ setup, onSkip }: { setup: OnboardingSetup; onSkip: () => void }) {
  const { agents, detected, hasAgent, providersProbed, providerVersions, providerPaths } = setup;
  const installs = useAppStore((s) => s.installs);
  const installAgent = useAppStore((s) => s.installAgent);

  return (
    <SetupStep
      num="03"
      eyebrow="Bring your own agent"
      title={
        <>
          Claude, Codex, Cursor — <em>under one roof.</em>
        </>
      }
      lede={
        <>
          Fletch directs the agent CLIs you already pay for — <b>no lock-in, ever.</b> Install at
          least one to run your first task; it's a single click.
        </>
      }
      points={[
        { icon: "refresh", head: "Swap per task.", body: "Pick the right model for the job." },
        {
          icon: "settings",
          head: "Your keys, your limits.",
          body: "Connects to your existing subscriptions.",
        },
      ]}
      exhibit={
        <div className="ob-exhibit-wrap ob-reveal" style={{ "--d": ".25s" } as CSSProperties}>
          <div className="ob-exhibit">
            <ExBar title="fletch — agents" />
            <div className="ob-ag-list">
              {agents.map((p) => {
                const d = PROVIDER_DETAIL[p.id];
                const path = providerPaths[p.id];
                const inst = installs[p.id];
                const ok = !!path;
                // Platform-aware: also gates the one-click button, mirroring
                // which agents the backend can actually script-install here.
                const cmd = installCommand(p.id);
                const cls = ok ? "ok" : inst?.phase === "failed" ? "failed" : "";
                let sub: React.ReactNode;
                if (ok) sub = d.signIn ?? d.models;
                else if (inst?.phase === "running") sub = inst.line ?? "installing…";
                else if (inst?.phase === "failed") sub = <span className="err">{inst.error}</span>;
                else sub = cmd ?? "install via the setup guide";
                return (
                  <div key={p.id} className={`ob-ag ${cls}`}>
                    <ProviderIcon slug={p.id} short={p.short} hue={p.hue} size={30} />
                    <span className="meta">
                      <span className="pl">{p.label}</span>
                      <span className="ps">{sub}</span>
                    </span>
                    <span className="ob-ag-act">
                      {ok ? (
                        <span className="ob-ag-ver">
                          <Icon name="check" size={11} strokeWidth={2} />
                          {providerVersions[p.id] ?? "installed"}
                        </span>
                      ) : inst?.phase === "running" ? (
                        <span className="ob-spinner" />
                      ) : inst?.phase === "failed" ? (
                        <>
                          {cmd && <CopyCmd cmd={cmd} />}
                          <button
                            type="button"
                            className="ob-ag-install"
                            onClick={() => void installAgent(p.id)}
                          >
                            Retry
                          </button>
                        </>
                      ) : cmd ? (
                        <button
                          type="button"
                          className="ob-ag-install"
                          onClick={() => void installAgent(p.id)}
                        >
                          Install
                        </button>
                      ) : (
                        <DocsLink url={d.docs} label="install" />
                      )}
                    </span>
                  </div>
                );
              })}
            </div>
            <div className="ob-exhibit-cap">
              <span className="lvdot" />
              {providersProbed ? `${detected} of ${agents.length} installed` : "detecting agents…"}{" "}
              · auto-detects as you install
            </div>
          </div>
        </div>
      }
    >
      <div className="ob-setup-card ob-reveal" style={{ "--d": ".46s" } as CSSProperties}>
        <div className="ob-setup-line">
          {hasAgent ? (
            <>
              <span className="ob-rdy-dot ok" />
              <span>
                <b>
                  {detected} agent{detected === 1 ? "" : "s"} detected
                </b>{" "}
                — you're good to go
              </span>
            </>
          ) : providersProbed ? (
            <>
              <span className="ob-rdy-dot bad" />
              <span>
                <b>No agents yet</b> — install one on the right to continue
              </span>
            </>
          ) : (
            <>
              <span className="ob-spinner" />
              <span>Scanning this machine for agent CLIs…</span>
            </>
          )}
        </div>
      </div>
      {!hasAgent && (
        <button
          type="button"
          className="ob-skiplink ob-reveal"
          style={{ "--d": ".56s" } as CSSProperties}
          onClick={onSkip}
        >
          Set up later
        </button>
      )}
    </SetupStep>
  );
}
