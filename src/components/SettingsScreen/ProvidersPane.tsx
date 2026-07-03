import { useState } from "react";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { PROVIDER_DETAIL } from "@/data/providerDetail";
import { PROVIDERS, type Provider } from "@/data/providers";
import { useAppStore } from "@/store";
import { BinaryPathRow } from "./BinaryPathRow";
import { SetGroup, SetHead, SetToggle } from "./primitives";

export function ProvidersPane() {
  const providerFlags = useAppStore((s) => s.providerFlags);
  const setProviderEnabled = useAppStore((s) => s.setProviderEnabled);
  const providerVersions = useAppStore((s) => s.providerVersions);
  const providerPaths = useAppStore((s) => s.providerPaths);
  const providerPathOverrides = useAppStore((s) => s.providerPathOverrides);
  const setProviderPathOverride = useAppStore((s) => s.setProviderPathOverride);
  const refreshProviderVersions = useAppStore((s) => s.refreshProviderVersions);
  const [scanning, setScanning] = useState(false);

  const enabledCount = PROVIDERS.filter((p) => providerFlags[p.id] !== false).length;

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
        desc={`${enabledCount} of ${PROVIDERS.length} agents enabled. Toggle an agent off to hide it from the composer's model picker without signing out.`}
        actions={
          <>
            {scanning && <span className="set-checked mono text-xs">Scanning…</span>}
            <IconButton
              tipDown
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

      <SetGroup label="Installed on this system" last>
        <div className="set-prov-list">
          {PROVIDERS.map((p) => (
            <ProviderRow
              key={p.id}
              provider={p}
              enabled={providerFlags[p.id] !== false}
              onToggle={() => setProviderEnabled(p.id, providerFlags[p.id] === false)}
              liveVersion={providerVersions[p.id]}
              livePath={providerPaths[p.id]}
              override={providerPathOverrides[p.id]}
              onSavePath={(path) => setProviderPathOverride(p.id, path)}
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
  override,
  onSavePath,
}: {
  provider: Provider;
  enabled: boolean;
  onToggle: () => void;
  liveVersion?: string;
  livePath?: string;
  override?: string;
  onSavePath: (path: string | null) => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const d = PROVIDER_DETAIL[provider.id];
  if (!d) return null;

  // What the binary row shows: an explicit override wins, then the live probe,
  // then the hardcoded default. The override leads even when not yet resolved
  // so a custom (and possibly broken) path stays visible to the user.
  const effectivePath = override ?? livePath ?? d.path;

  return (
    <div className={`set-prov ${enabled ? "" : "off"} ${open ? "open" : ""}`}>
      <div className="set-prov-main flex-center">
        <span
          className={`set-prov-status ${livePath ? "" : "none"}`}
          title={livePath ? "Detected on this system" : "Not found"}
        />
        <ProviderIcon slug={provider.id} short={provider.short} hue={provider.hue} />
        <div className="set-prov-id">
          <div className="set-prov-name flex-center text-base">
            {provider.label}
            <span className="set-prov-ver mono text-xs">{liveVersion}</span>
          </div>
          <div className="set-prov-sub flex-center truncate mono text-sm">{effectivePath}</div>
        </div>
        <button
          className={`set-prov-chev iflex-center ${open ? "open" : ""}`}
          aria-label={open ? "Collapse details" : "Expand details"}
          onClick={() => setOpen((v) => !v)}
        >
          <Icon name="chevD" size={13} />
        </button>
        <SetToggle on={enabled} onClick={onToggle} />
      </div>

      {open && (
        <div className="set-prov-detail">
          <BinaryPathRow
            providerLabel={provider.label}
            effectivePath={effectivePath}
            override={override}
            resolved={!!livePath}
            onSave={onSavePath}
          />
          <ProvDetailRow k="Models" v={d.models} />
          <div className="set-prov-detail-actions flex-center">
            <Button variant="ghost" size="sm">
              View logs
            </Button>
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
