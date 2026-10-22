// The agents panel: every supported CLI with its live install state, framed
// as a window of the product. While its row is checking it probes the tiles
// one at a time — a sweep down the list, each tile landing as found or not —
// so the "scanning" the row talks about is something the user watches
// happen. Afterwards it is the place to click Install, and it re-probes every
// few seconds while on screen, so an install finishing anywhere (here or in
// the user's own terminal) lights its tile up.

import { useEffect, useState } from "react";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { DocsLink } from "@/components/ui/DocsLink";
import { installCommand, PROVIDER_DETAIL } from "@/data/providerDetail";
import { useAppStore } from "@/store";
import { CopyCmd } from "../CopyCmd";
import { ExBar } from "../exhibits";
import type { OnboardingSetup } from "../useSetup";
import { SCAN_STEP_MS } from "./usePreflight";

export function AgentsPanel({ setup, scanning }: { setup: OnboardingSetup; scanning: boolean }) {
  const { agents, detected, providersProbed, providerVersions, providerPaths } = setup;
  const installs = useAppStore((s) => s.installs);
  const installAgent = useAppStore((s) => s.installAgent);

  // The sweep: `probed` is how many tiles have landed. It runs once, from the
  // first mount while the row is checking, and only advances past a tile once
  // the real probe has answered — a slow machine holds the sweep, it never
  // lies. A later mount (the user looking back at this row) shows the result.
  const [probed, setProbed] = useState(scanning ? 0 : agents.length);
  useEffect(() => {
    if (probed >= agents.length || !providersProbed) return;
    const t = window.setTimeout(() => setProbed((n) => n + 1), SCAN_STEP_MS);
    return () => window.clearTimeout(t);
  }, [probed, agents.length, providersProbed]);
  const sweeping = probed < agents.length;

  return (
    <div className="ob-exhibit-wrap">
      <div className="ob-exhibit">
        <ExBar title="fletch — agents" />
        <div className="ob-ag-list">
          {agents.map((p, i) => {
            const d = PROVIDER_DETAIL[p.id];
            const inst = installs[p.id];
            const ok = !!providerPaths[p.id];
            const cmd = installCommand(p.id);
            const busy = inst?.phase === "running" || inst?.phase === "cancelling";

            // During the sweep a tile is either not reached, being probed, or
            // landed; after it, the live install state takes over.
            const phase = i > probed ? "idle" : i === probed && sweeping ? "probing" : "landed";
            const cls =
              phase === "idle"
                ? "idle"
                : phase === "probing"
                  ? "probing"
                  : ok
                    ? "ok"
                    : inst?.phase === "failed"
                      ? "failed"
                      : "miss";

            let sub: React.ReactNode;
            if (phase !== "landed") sub = phase === "probing" ? "looking…" : "";
            else if (ok) sub = d.signIn ?? d.models;
            else if (inst?.phase === "running") sub = inst.line ?? "installing…";
            else if (inst?.phase === "cancelling") sub = "cancelling…";
            else if (inst?.phase === "failed") sub = <span className="err">{inst.error}</span>;
            else sub = cmd ?? "install via the setup guide";

            let action: React.ReactNode = null;
            if (phase === "probing") action = <span className="ob-spinner" />;
            else if (phase === "landed") {
              if (ok) {
                action = (
                  <span className="ob-ag-ver">
                    <Icon name="check" size={11} strokeWidth={2} />
                    {providerVersions[p.id] ?? "installed"}
                  </span>
                );
              } else if (busy) action = <span className="ob-spinner" />;
              else if (inst?.phase === "failed") {
                action = (
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
                );
              } else if (cmd) {
                action = (
                  <button
                    type="button"
                    className="ob-ag-install"
                    onClick={() => void installAgent(p.id)}
                  >
                    Install
                  </button>
                );
              } else action = <DocsLink url={d.docs} label="install" />;
            }

            return (
              <div key={p.id} className={`ob-ag ${cls}`}>
                <ProviderIcon slug={p.id} short={p.short} hue={p.hue} size={30} />
                <span className="meta">
                  <span className="pl">{p.label}</span>
                  <span className="ps">{sub}</span>
                </span>
                <span className="ob-ag-act">{action}</span>
              </div>
            );
          })}
        </div>
        <div className="ob-exhibit-cap">
          <span className={`lvdot ${sweeping ? "busy" : ""}`} />
          {sweeping
            ? `checking ${Math.min(probed + 1, agents.length)} of ${agents.length}…`
            : `${detected} of ${agents.length} installed · auto-detects as you install`}
        </div>
      </div>
    </div>
  );
}
