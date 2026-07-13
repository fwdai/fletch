// RunView/BudgetMeter.tsx — the run's budget meters (spec §14.2, §11). Reads the
// effective budgets and the spent ledger off the run row. The ledger is populated
// by the budgets slice (S5); until then turns/tokens read as 0 and wall-clock is
// derived live from the run's start — an honest, non-misleading partial view that
// becomes fully live when S5 lands.

import { useMinuteClock } from "../../../util/hooks";

/** Read a positive number field from an untyped JSON value. */
function num(v: unknown, key: string): number | undefined {
  if (v && typeof v === "object" && key in v) {
    const n = (v as Record<string, unknown>)[key];
    if (typeof n === "number" && Number.isFinite(n) && n > 0) return n;
  }
  return undefined;
}

interface Meter {
  label: string;
  used: number;
  cap: number;
  fmt: (n: number) => string;
}

const plain = (n: number) => n.toLocaleString();
const compact = (n: number) =>
  n >= 1000 ? `${(n / 1000).toFixed(n >= 10_000 ? 0 : 1)}k` : String(n);

export function BudgetMeter({
  budgets,
  spent,
  createdAt,
}: {
  budgets: unknown;
  spent: unknown;
  createdAt: number;
}) {
  // Re-render each minute so the live wall-clock meter advances (epoch ms).
  const now = useMinuteClock();

  const meters: Meter[] = [];

  const turns = num(budgets, "turns");
  if (turns)
    meters.push({ label: "turns", used: num(spent, "turns") ?? 0, cap: turns, fmt: plain });

  const tokens = num(budgets, "tokens");
  if (tokens)
    meters.push({ label: "tokens", used: num(spent, "tokens") ?? 0, cap: tokens, fmt: compact });

  const wallCap = num(budgets, "wall_clock_mins");
  if (wallCap) {
    const elapsed = Math.max(0, Math.round((now - createdAt) / 60_000));
    meters.push({ label: "minutes", used: elapsed, cap: wallCap, fmt: plain });
  }

  if (meters.length === 0) return null;

  return (
    <div className="wf-budget">
      {meters.map((m) => {
        const frac = Math.min(1, m.cap > 0 ? m.used / m.cap : 0);
        const near = frac >= 0.9;
        return (
          <div className="wf-budget-item" key={m.label} title={`${m.used} / ${m.cap} ${m.label}`}>
            <span className="wf-budget-label">{m.label}</span>
            <span className="wf-budget-track">
              <span
                className={`wf-budget-fill ${near ? "near" : ""}`}
                style={{ width: `${frac * 100}%` }}
              />
            </span>
            <span className="wf-budget-num mono">
              {m.fmt(m.used)}
              <span className="wf-budget-cap">/{m.fmt(m.cap)}</span>
            </span>
          </div>
        );
      })}
    </div>
  );
}
