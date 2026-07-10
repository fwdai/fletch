// Step 03 · Agents. The one hard gate: Fletch is unusable without at least
// one agent CLI, so Continue stays disabled until a probe finds one. Each
// missing agent offers a one-click install (the backend runs its official
// native installer — see src-tauri/src/agent_install.rs) with the exact
// command shown for transparency; while this step is on screen the shared
// setup hook re-probes every few seconds, so an install finishing — ours or
// one the user ran in their own terminal — lights the tile up by itself.

import { type CSSProperties, useEffect, useRef, useState } from "react";
import { type AgentInstallEvent, api, onAgentInstallState } from "@/api";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { PROVIDER_DETAIL } from "@/data/providerDetail";
import type { ProviderId } from "@/data/providers";
import { ExBar } from "./exhibits";
import { CopyCmd, DocsLink, SetupStep } from "./SetupBits";
import type { OnboardingSetup } from "./useSetup";

type InstallState = { phase: "running"; line?: string } | { phase: "failed"; error: string };

export function AgentsStep({ setup, onSkip }: { setup: OnboardingSetup; onSkip: () => void }) {
  const { agents, detected, hasAgent, providersProbed, providerVersions, providerPaths } = setup;
  const [installs, setInstalls] = useState<Partial<Record<ProviderId, InstallState>>>({});
  const installsRef = useRef(installs);
  installsRef.current = installs;

  // Stream installer output into the running tile's status line.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void onAgentInstallState((e: AgentInstallEvent) => {
      const id = e.id as ProviderId;
      if (e.phase !== "running" || installsRef.current[id]?.phase !== "running") return;
      setInstalls((m) => ({ ...m, [id]: { phase: "running", line: e.line } }));
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const install = (id: ProviderId) => {
    setInstalls((m) => ({ ...m, [id]: { phase: "running" } }));
    void api
      .installAgent(id)
      .then(() => setup.refreshProviders())
      .then(() =>
        setInstalls((m) => {
          const { [id]: _done, ...rest } = m;
          return rest;
        }),
      )
      .catch((err) =>
        setInstalls((m) => ({ ...m, [id]: { phase: "failed", error: String(err) } })),
      );
  };

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
                const cls = ok ? "ok" : inst?.phase === "failed" ? "failed" : "";
                let sub: React.ReactNode;
                if (ok) sub = d.signIn ?? d.models;
                else if (inst?.phase === "running") sub = inst.line ?? "installing…";
                else if (inst?.phase === "failed") sub = <span className="err">{inst.error}</span>;
                else sub = d.install ?? "install via the setup guide";
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
                          {d.install && <CopyCmd cmd={d.install} />}
                          <button
                            type="button"
                            className="ob-ag-install"
                            onClick={() => install(p.id)}
                          >
                            Retry
                          </button>
                        </>
                      ) : d.install ? (
                        <button
                          type="button"
                          className="ob-ag-install"
                          onClick={() => install(p.id)}
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
