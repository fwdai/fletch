// pickers.tsx — the builder's agent-picker popover, fixed-positioned (escaping
// the canvas clip) from a measured rect. Gate and budget editing moved inline
// into the inspector (see inspector/), so this is the one popover left.

import { PROVIDERS } from "../../data/providers";
import type { CustomAgent } from "../../storage/customAgents";
import { useAppStore } from "../../store";
import { defaultModel, shortFor } from "../shared";
import { AgentAvatar } from "./AgentAvatar";
import type { PopRect } from "./ctx";

export function AgentPick({
  rect,
  agents,
  onPick,
}: {
  rect: PopRect;
  agents: CustomAgent[];
  onPick: (id: string) => void;
}) {
  // Base agents = the coding-agent providers enabled in Settings → Providers.
  const providerFlags = useAppStore((s) => s.providerFlags);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const enabled = PROVIDERS.filter((p) => providerFlags[p.id] !== false);
  return (
    <div className="dd wb-pick" style={{ position: "fixed", top: rect.top, left: rect.left }}>
      <div className="dd-sect">Custom agents</div>
      {agents.length === 0 && (
        <div style={{ padding: "4px 9px 8px", fontSize: 11.5, color: "var(--fg-3)" }}>
          None yet — create some under Custom agents.
        </div>
      )}
      {agents.map((a) => (
        <div key={a.id} className="wb-pick-item" onClick={() => onPick(a.id)}>
          <AgentAvatar custom slug={a.base} short={shortFor(a.name)} hue={a.color} size={22} />
          <span className="wb-pick-l">
            <div className="wb-pick-n">{a.name}</div>
            <div className="wb-pick-m">
              {PROVIDERS.find((p) => p.id === a.base)?.label} · {a.model ?? "default"}
            </div>
          </span>
        </div>
      ))}
      <div className="dd-sep"></div>
      <div className="dd-sect">Base agents</div>
      {enabled.map((p) => (
        <div key={p.id} className="wb-pick-item" onClick={() => onPick(p.id)}>
          <AgentAvatar custom={false} slug={p.id} short={p.short} hue={p.hue} size={22} />
          <span className="wb-pick-l">
            <div className="wb-pick-n">{p.label}</div>
            <div className="wb-pick-m">{defaultModel(p.id, modelsByAgent) ?? "default model"}</div>
          </span>
        </div>
      ))}
    </div>
  );
}
