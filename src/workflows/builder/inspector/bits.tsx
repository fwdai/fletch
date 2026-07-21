// bits.tsx — small shared pieces of the inspector: labelled fields, the agent
// select button, and the budget field groups (formerly the budgets popover).

import { Icon } from "../../../components/Icon";
import type { ResolvedAgent } from "../../shared";
import type { Budgets } from "../../spec";
import { AgentAvatar } from "../AgentAvatar";

export function Field({
  label,
  hint,
  required,
  children,
}: {
  label: string;
  hint?: string;
  required?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div className="wb-field">
      <div className="wb-label">
        {label} {required && <span className="req">*</span>}
        {hint && <span className="hint">{hint}</span>}
      </div>
      {children}
    </div>
  );
}

/** The agent select button: identity when assigned, a dashed placeholder when
 *  not. Opens the agent picker popover via `onClick`. */
export function AgentButton({
  agent,
  placeholder,
  onClick,
}: {
  agent: ResolvedAgent | null;
  placeholder: string;
  onClick: (e: React.MouseEvent) => void;
}) {
  return (
    <button className="wb-agent-btn" onClick={onClick}>
      {agent ? (
        <AgentAvatar
          custom={agent.custom}
          slug={agent.providerId}
          short={agent.short}
          hue={agent.hue}
          size={28}
        />
      ) : (
        <span className="wb-step-mono empty">
          <Icon name="plus" size={12} />
        </span>
      )}
      <span className="wb-agent-btn-l">
        <div className={`wb-an ${agent ? "" : "empty"}`}>{agent ? agent.name : placeholder}</div>
        {agent && (
          <div className="wb-am">
            {agent.custom ? `${agent.baseLabel} · ${agent.model}` : agent.model}
          </div>
        )}
      </span>
      <Icon name="chevD" size={13} />
    </button>
  );
}

interface BudgetFieldDef {
  key: keyof Budgets;
  label: string;
  placeholder: string;
}

/** Curated per-scope fields; the rest of §11.1 (timeouts) keep their defaults. */
export const RUN_BUDGET_FIELDS: BudgetFieldDef[] = [
  { key: "turns", label: "Total turns", placeholder: "100" },
  { key: "tokens", label: "Total tokens", placeholder: "unlimited" },
  { key: "wall_clock_mins", label: "Wall-clock (min)", placeholder: "480" },
];
export const STEP_BUDGET_FIELDS: BudgetFieldDef[] = [
  { key: "turns", label: "Turns", placeholder: "—" },
  { key: "turns_per_attempt", label: "Turns / attempt", placeholder: "10" },
  { key: "max_attempts", label: "Max attempts", placeholder: "2" },
];

export function BudgetFields({
  fields,
  value,
  onChange,
}: {
  fields: BudgetFieldDef[];
  value: Budgets | undefined;
  onChange: (next: Budgets | undefined) => void;
}) {
  const set = (key: keyof Budgets, raw: string) => {
    const next: Budgets = { ...(value ?? {}) };
    const n = raw.trim() === "" ? undefined : Number(raw);
    if (n == null || Number.isNaN(n)) delete next[key];
    else next[key] = n;
    onChange(Object.keys(next).length ? next : undefined);
  };

  return (
    <div className="wb-budget-grid">
      {fields.map((f) => (
        <label className="wb-budget-field" key={f.key}>
          <span>{f.label}</span>
          <input
            className="ca-input sm"
            type="number"
            min={1}
            value={value?.[f.key] ?? ""}
            placeholder={f.placeholder}
            onChange={(e) => set(f.key, e.target.value)}
          />
        </label>
      ))}
      <div className="wb-field-note">
        Blank uses the app default. Exceeding a budget pauses the run — never a silent overspend.
      </div>
    </div>
  );
}
