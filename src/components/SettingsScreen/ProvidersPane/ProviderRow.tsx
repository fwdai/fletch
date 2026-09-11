// One provider row. Seven states — installed+enabled, installed+disabled,
// missing, installing, cancelling, installed-just-now ("fresh") and failed —
// all sharing the same geometry: only the dot, the sub-line, the chips and the
// right-hand action change, so a row never moves under the cursor while it
// changes state.
//
// `cancelling` wears the installing look on purpose: the install is still
// being torn down, and offering Install or Retry before the backend says it is
// gone would race the installer the user just stopped.

import { type ReactNode, useEffect, useState } from "react";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Button } from "@/components/ui/Button";
import { DocsLink } from "@/components/ui/DocsLink";
import { modelSummary } from "@/data/modelCatalog";
import { installCommand, PROVIDER_DETAIL } from "@/data/providerDetail";
import type { Provider } from "@/data/providers";
import { useAppStore } from "@/store";
import type { InstallState } from "@/store/types";
import { BinaryPathRow } from "../BinaryPathRow";
import { ProviderAuthBadge } from "../ProviderAuthBadge";
import { SetToggle } from "../primitives";
import { InstallLog } from "./InstallLog";
import { InstallOptions } from "./InstallOptions";
import { SignInSection } from "./SignInSection";

type RowState = "installed" | "fresh" | "missing" | "installing" | "cancelling" | "failed";

export function ProviderRow({ provider }: { provider: Provider }) {
  const { id, label } = provider;
  const enabled = useAppStore((s) => s.providerFlags[id] !== false);
  const liveVersion = useAppStore((s) => s.providerVersions[id]);
  const livePath = useAppStore((s) => s.providerPaths[id]);
  const override = useAppStore((s) => s.providerPathOverrides[id]);
  const install = useAppStore((s) => s.installs[id]);
  const auth = useAppStore((s) => s.providerAuth[id]);
  const liveModels = useAppStore((s) => s.modelsByAgent[id]);
  const setProviderEnabled = useAppStore((s) => s.setProviderEnabled);
  const setProviderPathOverride = useAppStore((s) => s.setProviderPathOverride);
  const installAgent = useAppStore((s) => s.installAgent);
  const cancelAgentInstall = useAppStore((s) => s.cancelAgentInstall);
  const clearInstallState = useAppStore((s) => s.clearInstallState);

  const [open, setOpen] = useState(false);
  const phase = install?.phase;

  // The live log is the point of the row while an installer runs (or is being
  // stopped), so open the detail for it. Not forced: the user can still
  // collapse it.
  useEffect(() => {
    if (phase === "running" || phase === "cancelling") setOpen(true);
  }, [phase]);

  const d = PROVIDER_DETAIL[id];
  if (!d) return null;

  // A run in flight outranks everything; after that the probe wins, so a
  // binary that turned up anyway — the user installed it in their own terminal,
  // or pointed us at one — clears a stale failure instead of contradicting it.
  // `fresh` is just "present, and we installed it a moment ago".
  const state: RowState =
    phase === "running"
      ? "installing"
      : phase === "cancelling"
        ? "cancelling"
        : livePath
          ? phase === "fresh"
            ? "fresh"
            : "installed"
          : phase === "failed"
            ? "failed"
            : "missing";
  const live = state === "installed" || state === "fresh";
  // A run in flight, one way or the other — and the two look alike.
  const busy = state === "installing" || state === "cancelling";

  // Platform-aware, and the same source the backend installs from: no command
  // here means there's no one-click install to offer (antigravity, pi).
  const cmd = installCommand(id);

  // What the binary row shows: an explicit override wins, then the live probe,
  // then the hardcoded default. The override leads even when not yet resolved
  // so a custom (and possibly broken) path stays visible to the user.
  const effectivePath = override ?? livePath ?? d.path;

  // Built once and handed to whichever detail is showing: "Locate binary…" is
  // the same validated editor in the installed, missing and failed states.
  const locate: ReactNode = (
    <BinaryPathRow
      providerLabel={label}
      effectivePath={effectivePath}
      override={override}
      resolved={!!livePath}
      onSave={(path) => setProviderPathOverride(id, path)}
    />
  );

  const toggle = () => {
    // First use of the toggle is also the acknowledgement of "Installed just
    // now" — drop the chip rather than leave it up until the pane closes.
    clearInstallState(id);
    setProviderEnabled(id, !enabled);
  };

  const sub = subLine(state, install, effectivePath);

  return (
    <div
      // `installed` is the row's default look and carries no class of its own;
      // `cancelling` borrows `installing`'s, being the same run winding down.
      className={`set-prov ${live && !enabled ? "off" : ""} ${
        state === "installed" ? "" : busy ? "installing" : state
      } ${open ? "open" : ""}`}
    >
      <div className="set-prov-main flex-center">
        <span className={`set-prov-status ${DOT[state].cls}`} title={DOT[state].title} />
        <ProviderIcon slug={id} short={provider.short} hue={provider.hue} />
        <div className="set-prov-id">
          <div className="set-prov-name flex-center text-base">
            {label}
            {live && (
              <span className={`set-prov-ver mono text-xs ${state === "fresh" ? "new" : ""}`}>
                {liveVersion}
              </span>
            )}
            {state === "missing" && (
              <span className="set-badge none mono text-xs">Not installed</span>
            )}
            {state === "fresh" && (
              <span className="set-badge ok mono text-xs">Installed just now</span>
            )}
            {/* A binary is not an account: only a row with a live binary can
                say anything about sign-in, and only when the probe classified
                it — `unknown` renders nothing rather than a guess. */}
            {live && <ProviderAuthBadge status={auth} />}
          </div>
          <div className={`set-prov-sub flex-center truncate mono text-sm ${sub.tone}`}>
            {sub.text}
          </div>
        </div>

        {state === "missing" &&
          (cmd ? (
            <Button variant="outline" size="sm" onClick={() => void installAgent(id)}>
              <Icon name="arrowDown" size={12} />
              Install
            </Button>
          ) : (
            <DocsLink url={d.docs} label="Install guide" />
          ))}
        {state === "installing" && (
          <Button variant="ghost" size="sm" onClick={() => void cancelAgentInstall(id)}>
            Cancel
          </Button>
        )}
        {/* Still the same button, just not clickable twice: the teardown is
            already under way and there is nothing else to ask for. */}
        {state === "cancelling" && (
          <Button variant="ghost" size="sm" disabled>
            Cancelling…
          </Button>
        )}
        {state === "failed" && (
          <Button variant="outline" size="sm" onClick={() => void installAgent(id)}>
            <Icon name="refresh" size={12} />
            Retry
          </Button>
        )}

        <button
          type="button"
          className={`set-prov-chev iflex-center ${open ? "open" : ""}`}
          aria-label={open ? "Collapse details" : "Expand details"}
          onClick={() => setOpen((v) => !v)}
        >
          <Icon name="chevD" size={13} />
        </button>
        {/* Providers are enabled by default, so the toggle shows its stored
            value even while the agent is missing — just not editable, since
            enabling something that can't run is a lie. */}
        <SetToggle on={enabled} onClick={toggle} disabled={!live} tip={TOGGLE_TIP[state]} />
      </div>

      {/* Vendor installers report no percentages, so the hairline is
          indeterminate rather than a faked progress bar. */}
      {busy && <div className="set-prov-bar" />}

      {open && (
        <div className="set-prov-detail">
          {state === "missing" && (
            <InstallOptions
              providerLabel={label}
              command={cmd}
              docs={d.docs}
              onInstall={() => void installAgent(id)}
              locate={locate}
            />
          )}
          {(busy || state === "failed") && (
            <InstallLog
              command={cmd}
              log={install && "log" in install ? install.log : []}
              error={install?.phase === "failed" ? install.error : undefined}
              onRetry={() => void installAgent(id)}
              locate={locate}
            />
          )}
          {live && (
            <>
              {locate}
              {/* Live-discovered models (same curated list as the composer's
                  picker), falling back to the static description until the
                  catalog has data for this agent. */}
              <ProvDetailRow k="Models" v={modelSummary(liveModels) ?? d.models} />
              {state === "fresh" && (
                <p className="set-prov-hint ok text-sm">
                  {enabled
                    ? `Installed. ${label} is live in the composer's model picker.`
                    : `Installed. Flip the toggle to show ${label} in the composer's model picker.`}
                </p>
              )}
              <SignInSection providerId={id} providerLabel={label} />
            </>
          )}
        </div>
      )}
    </div>
  );
}

/** Dot colour + hover title per row state. Green (no class) is "detected". */
const DOT: Record<RowState, { cls: string; title: string }> = {
  installed: { cls: "", title: "Detected on this system" },
  fresh: { cls: "", title: "Detected on this system" },
  missing: { cls: "none", title: "Not installed" },
  installing: { cls: "busy", title: "Installing" },
  cancelling: { cls: "busy", title: "Cancelling" },
  failed: { cls: "bad", title: "Install failed" },
};

/** Why the enable toggle is locked. Only ever shown while it is disabled. */
const TOGGLE_TIP: Record<RowState, string> = {
  installed: "Install first",
  fresh: "Install first",
  missing: "Install first",
  installing: "Installing…",
  cancelling: "Cancelling…",
  failed: "Install first",
};

/** The one line under the name: the binary path when there is one, and
 *  otherwise whatever the row is doing instead — never blank, never moved.
 *  While installing it mirrors the installer's latest output line; on failure
 *  it is the real exit line, never a generic apology. */
function subLine(
  state: RowState,
  install: InstallState | undefined,
  path: string,
): { text: string; tone: string } {
  if (state === "missing") {
    return { text: "Not found on PATH · install to enable", tone: "muted" };
  }
  if (state === "installing") {
    const line = install?.phase === "running" ? install.line : undefined;
    return { text: line ?? "starting installer…", tone: "" };
  }
  if (state === "cancelling") {
    return { text: "Cancelling…", tone: "" };
  }
  if (state === "failed") {
    const error = install?.phase === "failed" ? install.error : undefined;
    return { text: error ?? "install failed", tone: "bad" };
  }
  return { text: path, tone: "" };
}

export function ProvDetailRow({ k, v, mono }: { k: string; v: string; mono?: boolean }) {
  return (
    <div className="set-prov-drow">
      <span className="set-prov-dk">{k}</span>
      <span className={`set-prov-dv ${mono ? "mono" : ""}`}>{v}</span>
    </div>
  );
}
