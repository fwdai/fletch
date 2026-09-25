// Settings › Providers — every agent the app knows about, installed or not.
//
// A missing agent is a row with an Install button, not an absence: a user who
// skipped onboarding with nothing installed has to be able to get an agent CLI
// from here without opening a terminal. The install itself runs through the
// shared store slice (store/agentInstall.ts), so a run started in onboarding
// and one started here are the same run.

import { useEffect, useState } from "react";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { PROVIDERS } from "@/data/providers";
import { useAppStore } from "@/store";
import { activeEntry } from "@/store/capabilities";
import { useProviderPoll } from "@/util/useProviderPoll";
import { SetGroup, SetHead, SetRow, SetToggle } from "../primitives";
import { ProviderRow } from "./ProviderRow";

export function ProvidersPane() {
  const providerFlags = useAppStore((s) => s.providerFlags);
  const providerPaths = useAppStore((s) => s.providerPaths);
  const providersProbed = useAppStore((s) => s.providersProbed);
  const refreshProviderVersions = useAppStore((s) => s.refreshProviderVersions);
  const env = useAppStore(activeEntry);
  const [scanning, setScanning] = useState(false);

  // Keep re-probing while the pane is open, so an install the user runs in
  // their own terminal lights its row up on its own (same cadence as the
  // onboarding agents step).
  useProviderPoll();

  // "Installed just now" is a one-time confirmation, not state worth keeping:
  // drop it when the user leaves the pane. Read at cleanup time so the effect
  // doesn't re-run (and clear a live chip) on every install change.
  useEffect(
    () => () => {
      const { installs, clearInstallState } = useAppStore.getState();
      for (const [id, state] of Object.entries(installs)) {
        if (state.phase === "fresh") clearInstallState(id);
      }
    },
    [],
  );

  const rescan = async () => {
    if (scanning) return;
    setScanning(true);
    try {
      await refreshProviderVersions();
    } finally {
      setScanning(false);
    }
  };

  // Counts are about what's actually on this machine — "of 6 agents enabled"
  // read as if all six were usable. Enabling is only meaningful for an agent
  // that exists, so the denominator is the installed set.
  const installed = PROVIDERS.filter((p) => !!providerPaths[p.id]);
  const enabledCount = installed.filter((p) => providerFlags[p.id] !== false).length;
  const missingCount = PROVIDERS.length - installed.length;
  const counts = `${enabledCount} of ${installed.length} installed agents enabled${
    missingCount > 0 ? ` · ${missingCount} not installed` : ""
  }.`;

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Providers"
        title="Providers"
        desc={
          providersProbed
            ? `${counts} Toggle an agent off to hide it from the composer's model picker without signing out.`
            : "Scanning this machine for agent CLIs…"
        }
        actions={
          <>
            {scanning && <span className="set-checked mono text-xs">Scanning…</span>}
            <IconButton
              tip="Re-scan system"
              aria-label="Re-scan system"
              onClick={rescan}
              disabled={scanning}
            >
              <Icon name="refresh" />
            </IconButton>
          </>
        }
      />

      {/* This pane is This Mac's, whatever environment is active: it probes
          this machine's binaries and installs and signs in here, and none of
          those ops is on the wire (docs/remote-protocol.md). Say so rather
          than let the rows read as the host's. */}
      {env.kind === "remote" && (
        <p className="set-prov-hint text-sm">
          These are this Mac's providers. {env.name}'s are managed on {env.name}.
        </p>
      )}

      <SetGroup label="Agents on this system">
        <div className="set-prov-list">
          {PROVIDERS.map((p) => (
            <ProviderRow key={p.id} provider={p} />
          ))}
        </div>
      </SetGroup>

      <SetGroup label="Commits & pull requests" last>
        <AttributionRow />
      </SetGroup>
    </div>
  );
}

/** Two states, each named in the subtitle. PRs read the setting as they open;
 *  an agent's commit hook takes it from the env it started with. */
function AttributionRow() {
  const removed = useAppStore((s) => s.agentAttributionRemoved);
  const setRemoved = useAppStore((s) => s.setAgentAttributionRemoved);
  return (
    <SetRow
      title="Remove agent attribution"
      sub={`${
        removed
          ? "Removed. Fletch strips agents' Co-Authored-By trailers and “Generated with” footers from commits and PRs, whatever the agents' own settings say."
          : "Following your agent settings. Commits and PRs carry whatever attribution your agents' own settings add."
      } Applies to PRs right away, and to an agent's commits from its next start.`}
    >
      <SetToggle on={removed} onClick={() => void setRemoved(!removed)} />
    </SetRow>
  );
}
