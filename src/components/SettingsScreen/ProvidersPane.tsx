import { useState } from "react";
import { useAppStore } from "../../store";
import { PROVIDERS, type Provider } from "../../data/providers";
import { PROVIDER_DETAIL } from "../../data/providerDetail";
import { Icon } from "../Icon";
import { ProviderIcon } from "../ProviderIcon";
import { SetHead, SetGroup, SetToggle } from "./primitives";

export function ProvidersPane() {
  const providerFlags = useAppStore((s) => s.providerFlags);
  const setProviderEnabled = useAppStore((s) => s.setProviderEnabled);
  const providerVersions = useAppStore((s) => s.providerVersions);
  const providerPaths = useAppStore((s) => s.providerPaths);
  const refreshProviderVersions = useAppStore((s) => s.refreshProviderVersions);
  const [scanning, setScanning] = useState(false);

  const installed = PROVIDERS.filter((p) => PROVIDER_DETAIL[p.id]?.installed);
  const enabledCount = installed.filter((p) => providerFlags[p.id] !== false).length;

  const rescan = async () => {
    if (scanning) return;
    setScanning(true);
    try {
      await refreshProviderVersions();
    } finally {
      setScanning(false);
    }
  };

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Providers"
        title="Providers"
        desc={`${enabledCount} of ${installed.length} installed agents enabled. Toggle an agent off to hide it from the composer's model picker without signing out.`}
        actions={
          <>
            {scanning && <span className="set-checked mono">Scanning…</span>}
            <button
              className="btn-i tip"
              data-tip-down
              data-tip="Coming soon"
              aria-label="Add provider"
              disabled
            >
              <Icon name="plus" />
            </button>
            <button
              className="btn-i tip"
              data-tip-down
              data-tip="Re-scan system"
              aria-label="Re-scan system"
              onClick={rescan}
              disabled={scanning}
            >
              <Icon name="refresh" />
            </button>
          </>
        }
      />

      <SetGroup label="Installed on this system" last>
        <div className="set-prov-list">
          {installed.map((p) => (
            <ProviderRow
              key={p.id}
              provider={p}
              enabled={providerFlags[p.id] !== false}
              onToggle={() => setProviderEnabled(p.id, providerFlags[p.id] === false)}
              liveVersion={providerVersions[p.id]}
              livePath={providerPaths[p.id]}
            />
          ))}
        </div>
      </SetGroup>

    </div>
  );
}

function ProviderRow({
  provider,
  enabled,
  onToggle,
  liveVersion,
  livePath,
}: {
  provider: Provider;
  enabled: boolean;
  onToggle: () => void;
  liveVersion?: string;
  livePath?: string;
}) {
  const [open, setOpen] = useState(false);
  const d = PROVIDER_DETAIL[provider.id];
  if (!d) return null;

  return (
    <div className={`set-prov ${enabled ? "" : "off"} ${open ? "open" : ""}`}>
      <div className="set-prov-main">
        <span className="set-prov-status" title="Authenticated" />
        <ProviderIcon slug={provider.id} short={provider.short} hue={provider.hue} />
        <div className="set-prov-id">
          <div className="set-prov-name">
            {provider.label}
            <span className="set-prov-ver mono">{liveVersion ?? provider.version}</span>
          </div>
          <div className="set-prov-sub mono">{livePath ?? d.path}</div>
        </div>
        <button
          className={`set-prov-chev ${open ? "open" : ""}`}
          aria-label={open ? "Collapse details" : "Expand details"}
          onClick={() => setOpen((v) => !v)}
        >
          <Icon name="chevD" size={13} />
        </button>
        <SetToggle on={enabled} onClick={onToggle} />
      </div>

      {open && (
        <div className="set-prov-detail">
          <ProvDetailRow k="Binary" v={livePath ?? d.path} mono />
          <ProvDetailRow k="Models" v={d.models} />
          <div className="set-prov-detail-actions">
            <button className="btn-t ghost sm-t">View logs</button>
          </div>
        </div>
      )}
    </div>
  );
}

function ProvDetailRow({ k, v, mono }: { k: string; v: string; mono?: boolean }) {
  return (
    <div className="set-prov-drow">
      <span className="set-prov-dk">{k}</span>
      <span className={`set-prov-dv ${mono ? "mono" : ""}`}>{v}</span>
    </div>
  );
}

