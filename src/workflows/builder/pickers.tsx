// pickers.tsx — the builder popovers: agent picker, gate picker, budgets editor.
// All are fixed-positioned (escaping the canvas clip) from a measured rect.

import { Icon } from "../../components/Icon";
import { PROVIDERS } from "../../data/providers";
import type { CustomAgent } from "../../storage/customAgents";
import { useAppStore } from "../../store";
import { GATE_MODES, type GateKind } from "../data";
import { defaultModel, shortFor } from "../shared";
import type { Budgets } from "../spec";
import { AgentAvatar } from "./AgentAvatar";
import type { PopRect } from "./ctx";

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

// ── gate picker ───────────────────────────────────────────────────────────
export function GatePick({
  rect,
  gate,
  onPick,
}: {
  rect: PopRect;
  gate: GateKind;
  onPick: (kind: GateKind) => void;
}) {
  return (
    <div className="dd" style={{ position: "fixed", top: rect.top, left: rect.left, width: 300 }}>
      <div className="dd-sect">This step is done when…</div>
      {GATE_MODES.map((m) => (
        <div
          key={m.id}
          className={`dd-item ${m.id === gate ? "active" : ""}`}
          onClick={() => onPick(m.id)}
          style={{ alignItems: "flex-start", flexDirection: "column", gap: 3 }}
        >
          <span style={{ display: "flex", alignItems: "center", gap: 8, width: "100%" }}>
            <Icon name={m.icon} size={12} /> <span style={{ fontWeight: 500 }}>{m.label}</span>
            {m.id === gate && <Icon name="check" size={12} style={{ marginLeft: "auto" }} />}
          </span>
          <span style={{ fontSize: 11, color: "var(--fg-2)", lineHeight: 1.45, paddingLeft: 20 }}>
            {m.note}
          </span>
        </div>
      ))}
    </div>
  );
}

// ── budgets editor ──────────────────────────────────────────────────────────

interface BudgetFieldDef {
  key: keyof Budgets;
  label: string;
  placeholder: string;
}

/** Curated per-scope fields; the rest of §11.1 (timeouts) keep their defaults. */
const RUN_FIELDS: BudgetFieldDef[] = [
  { key: "turns", label: "Total turns", placeholder: "100" },
  { key: "tokens", label: "Total tokens", placeholder: "unlimited" },
  { key: "wall_clock_mins", label: "Wall-clock (min)", placeholder: "480" },
];
const STEP_FIELDS: BudgetFieldDef[] = [
  { key: "turns", label: "Turns", placeholder: "—" },
  { key: "turns_per_attempt", label: "Turns / attempt", placeholder: "10" },
  { key: "max_attempts", label: "Max attempts", placeholder: "2" },
];

export function BudgetsPopover({
  rect,
  scope,
  value,
  onChange,
}: {
  rect: PopRect;
  scope: "run" | "step";
  value: Budgets | undefined;
  onChange: (next: Budgets | undefined) => void;
}) {
  const fields = scope === "run" ? RUN_FIELDS : STEP_FIELDS;
  const left = Math.max(12, rect.left - 60);

  const set = (key: keyof Budgets, raw: string) => {
    const next: Budgets = { ...(value ?? {}) };
    const n = raw.trim() === "" ? undefined : Number(raw);
    if (n == null || Number.isNaN(n)) delete next[key];
    else next[key] = n;
    onChange(Object.keys(next).length ? next : undefined);
  };

  return (
    <div className="wb-loop-pop" style={{ position: "fixed", top: rect.top, left, width: 240 }}>
      <div className="wlp-h">{scope === "run" ? "Run budgets" : "Step budgets"}</div>
      {fields.map((f) => (
        <div className="wlp-row" key={f.key}>
          <label>{f.label}</label>
          <input
            className="ca-input"
            type="number"
            min={1}
            value={value?.[f.key] ?? ""}
            placeholder={f.placeholder}
            onChange={(e) => set(f.key, e.target.value)}
          />
        </div>
      ))}
      <div style={{ fontSize: 11, color: "var(--fg-3)", lineHeight: 1.45, marginTop: 8 }}>
        Blank uses the app default. Exceeding a budget pauses the run — never a silent overspend.
      </div>
    </div>
  );
}
