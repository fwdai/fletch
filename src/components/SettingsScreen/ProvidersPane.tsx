import { useState } from "react";
import { useAppStore } from "../../store";
import { PROVIDERS, type Provider } from "../../data/providers";
import {
  PROVIDER_DETAIL,
  AVAILABLE_AGENTS,
  type AvailableAgent,
} from "../../data/providerDetail";
import { Icon } from "../Icon";
import { ProviderIcon } from "../ProviderIcon";
import { SetHead, SetGroup, SetToggle } from "./primitives";

export function ProvidersPane() {
  const providerFlags = useAppStore((s) => s.providerFlags);
  const setProviderEnabled = useAppStore((s) => s.setProviderEnabled);

  const installed = PROVIDERS.filter((p) => PROVIDER_DETAIL[p.id]?.installed);
  const enabledCount = installed.filter((p) => providerFlags[p.id] !== false).length;

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Providers"
        title="Providers"
        desc={`${enabledCount} of ${installed.length} installed agents enabled. Toggle an agent off to hide it from the composer's model picker without signing out.`}
        actions={
          <>
            <span className="set-checked mono">Checked just now</span>
            <button className="btn-i tip" data-tip-down data-tip="Add provider" aria-label="Add provider">
              <Icon name="plus" />
            </button>
            <button className="btn-i tip" data-tip-down data-tip="Re-scan system" aria-label="Re-scan system">
              <Icon name="refresh" />
            </button>
          </>
        }
      />

      <SetGroup label="Installed on this system">
        <div className="set-prov-list">
          {installed.map((p) => (
            <ProviderRow
              key={p.id}
              provider={p}
              enabled={providerFlags[p.id] !== false}
              onToggle={() => setProviderEnabled(p.id, providerFlags[p.id] === false)}
            />
          ))}
        </div>
      </SetGroup>

      <SetGroup label="Available" last>
        <div className="set-prov-list">
          {AVAILABLE_AGENTS.map((a) => (
            <AvailableRow key={a.id} agent={a} />
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
}: {
  provider: Provider;
  enabled: boolean;
  onToggle: () => void;
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
            {d.earlyAccess && <span className="set-badge ea">Early Access</span>}
            <span className="set-prov-ver mono">{provider.version}</span>
            {d.update && (
              <span
                className="set-prov-update tip"
                data-tip-down
                data-tip={`Update to ${d.update}`}
              >
                <Icon name="arrowUp" size={11} />
              </span>
            )}
          </div>
          <div className="set-prov-sub">
            Authenticated as <span className="set-prov-acct mono">{d.account}</span>
            <span className="set-dot">·</span>
            <span>{d.plan}</span>
          </div>
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
          <ProvDetailRow k="Binary" v={d.path} mono />
          <ProvDetailRow k="Models" v={d.models} />
          <ProvDetailRow k="Plan" v={d.plan} />
          <div className="set-prov-detail-actions">
            <button className="btn-t outline sm-t">Re-authenticate</button>
            <button className="btn-t ghost sm-t">View logs</button>
            <span className="grow" />
            <button className="btn-t ghost sm-t danger">Sign out</button>
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

function AvailableRow({ agent }: { agent: AvailableAgent }) {
  const detected = agent.state === "detected";
  const soon = agent.state === "soon";
  return (
    <div className="set-prov available">
      <div className="set-prov-main">
        <span
          className={`set-prov-status ${detected ? "idle" : "none"}`}
          title={detected ? "Detected" : "Not installed"}
        />
        <ProviderIcon slug={agent.id} short={agent.short} hue={agent.hue} />
        <div className="set-prov-id">
          <div className="set-prov-name">
            {agent.label}
            {soon && <span className="set-badge soon">Coming soon</span>}
            {agent.version && <span className="set-prov-ver mono">{agent.version}</span>}
          </div>
          <div className="set-prov-sub muted">{agent.note}</div>
        </div>
        {detected && <button className="btn-t outline sm-t">Configure</button>}
        {agent.state === "install" && (
          <button className="btn-t ghost sm-t">
            Install <Icon name="external" size={11} />
          </button>
        )}
      </div>
    </div>
  );
}
