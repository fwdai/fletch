// pickers.tsx — the three builder popovers: agent, advance-when, loop editor.
// All are fixed-positioned (escaping the canvas clip) from a measured rect.

import { useState } from "react";
import { Icon } from "../../components/Icon";
import { PROVIDERS } from "../../data/providers";
import type { CustomAgent } from "../../storage/customAgents";
import { useAppStore } from "../../store";
import { ADVANCE_MODES } from "../data";
import type { AgentResolver } from "../shared";
import { defaultModel, shortFor } from "../shared";
import type { AdvanceMode, WorkflowStep, WorkflowStepLoop } from "../storage";
import { AgentAvatar } from "./AgentAvatar";

export interface PopRect {
  top: number;
  left: number;
  right: number;
  bottom: number;
}

// ── agent picker ──────────────────────────────────────────────────────────
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

// ── advance-when picker ───────────────────────────────────────────────────
export function AdvancePick({
  rect,
  value,
  onPick,
}: {
  rect: PopRect;
  value: AdvanceMode | undefined;
  onPick: (v: AdvanceMode) => void;
}) {
  return (
    <div className="dd" style={{ position: "fixed", top: rect.top, left: rect.left, width: 280 }}>
      <div className="dd-sect">Advance to next step when…</div>
      {ADVANCE_MODES.map((m) => (
        <div
          key={m.id}
          className={`dd-item ${m.id === value ? "active" : ""}`}
          onClick={() => onPick(m.id)}
          style={{ alignItems: "flex-start", flexDirection: "column", gap: 3 }}
        >
          <span style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <Icon name={m.icon} size={12} /> <span style={{ fontWeight: 500 }}>{m.label}</span>
            {m.id === value && <Icon name="check" size={12} style={{ marginLeft: "auto" }} />}
          </span>
          <span style={{ fontSize: 11, color: "var(--fg-2)", lineHeight: 1.45, paddingLeft: 20 }}>
            {m.note}
          </span>
        </div>
      ))}
    </div>
  );
}

// ── loop editor ───────────────────────────────────────────────────────────
export function LoopEditor({
  rect,
  step,
  steps,
  resolve,
  onApply,
  onClear,
}: {
  rect: PopRect;
  step: WorkflowStep;
  steps: WorkflowStep[];
  resolve: AgentResolver;
  onApply: (loop: WorkflowStepLoop) => void;
  onClear: () => void;
}) {
  const earlier = steps.slice(
    0,
    steps.findIndex((s) => s.id === step.id),
  );
  const [to, setTo] = useState(step.loop?.to || earlier[earlier.length - 1]?.id || "");
  const [when, setWhen] = useState(step.loop?.when || "review requests changes");
  const [max, setMax] = useState(step.loop?.max || 3);

  const left = Math.max(12, rect.left - 120);

  if (earlier.length === 0) {
    return (
      <div className="wb-loop-pop" style={{ position: "fixed", top: rect.top, left }}>
        <div className="wlp-h">Loop</div>
        <div style={{ fontSize: 12, color: "var(--fg-2)", lineHeight: 1.5 }}>
          A loop returns to an earlier step. Add a step before this one first.
        </div>
      </div>
    );
  }

  return (
    <div className="wb-loop-pop" style={{ position: "fixed", top: rect.top, left }}>
      <div className="wlp-h">Loop back</div>
      <div className="wlp-row">
        <label>Return to step</label>
        <select className="ca-select" value={to} onChange={(e) => setTo(e.target.value)}>
          {earlier.map((s) => {
            const a = resolve(s.agent);
            const idx = steps.findIndex((x) => x.id === s.id) + 1;
            return (
              <option key={s.id} value={s.id}>
                {String(idx).padStart(2, "0")} · {a?.name || "Unassigned"}
              </option>
            );
          })}
        </select>
      </div>
      <div className="wlp-row">
        <label>Condition</label>
        <input
          className="ca-input"
          value={when}
          onChange={(e) => setWhen(e.target.value)}
          placeholder="e.g. review requests changes"
        />
      </div>
      <div className="wlp-row">
        <label>Max iterations</label>
        <select
          className="ca-select"
          value={max}
          onChange={(e) => setMax(parseInt(e.target.value, 10))}
        >
          {[1, 2, 3, 4, 5].map((n) => (
            <option key={n} value={n}>
              {n}×
            </option>
          ))}
        </select>
      </div>
      <div style={{ display: "flex", gap: 8, marginTop: 13 }}>
        {step.loop && (
          <button className="btn-t ghost sm-t" onClick={onClear}>
            Remove
          </button>
        )}
        <span style={{ flex: 1 }}></span>
        <button
          className="btn-t primary"
          style={{ height: 28 }}
          onClick={() => onApply({ to, when, max })}
        >
          {step.loop ? "Update" : "Add loop"}
        </button>
      </div>
    </div>
  );
}
