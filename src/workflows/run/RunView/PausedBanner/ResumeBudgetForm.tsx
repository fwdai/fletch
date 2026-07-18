import { useState } from "react";
import { api, type WfRun } from "../../../../api";
import { Icon } from "../../../../components/Icon";
import type { Budgets } from "../../../spec";

/** Does the run have a token cap set? Token patches are ignored when the run's
 *  token budget is unlimited (§11.2), so the field is only worth showing then. */
function hasTokenCap(budgets: unknown): boolean {
  if (!budgets || typeof budgets !== "object" || !("tokens" in budgets)) return false;
  const t = (budgets as { tokens: unknown }).tokens;
  return typeof t === "number" && t > 0;
}

/** Inline "raise the budget and resume" form for a `paused(budget_exceeded)`
 *  run (§11.2). Each field is an additive bump to a run-level cap; resuming with
 *  no bump would just re-hit the same budget, so at least one is required. */
export function ResumeBudgetForm({ run, onError }: { run: WfRun; onError: (m: string) => void }) {
  const [turns, setTurns] = useState("");
  const [tokens, setTokens] = useState("");
  const [minutes, setMinutes] = useState("");
  const [busy, setBusy] = useState(false);
  const showTokens = hasTokenCap(run.budgets);

  const parse = (s: string): number | undefined => {
    const n = Math.floor(Number(s));
    return Number.isFinite(n) && n > 0 ? n : undefined;
  };
  const patch: Budgets = {
    turns: parse(turns),
    tokens: showTokens ? parse(tokens) : undefined,
    wall_clock_mins: parse(minutes),
  };
  const hasBump = patch.turns != null || patch.tokens != null || patch.wall_clock_mins != null;

  const resume = async () => {
    if (busy || !hasBump) return;
    setBusy(true);
    try {
      await api.wfResume(run.id, patch);
      // The `wf:run` subscription flips the run back to running on success.
    } catch (e) {
      onError(`Resume failed: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const field = (label: string, value: string, set: (v: string) => void) => (
    <label className="wf-budget-field">
      <span>+ {label}</span>
      <input
        type="number"
        min="0"
        step="1"
        inputMode="numeric"
        placeholder="0"
        value={value}
        disabled={busy}
        onChange={(e) => set(e.target.value)}
      />
    </label>
  );

  return (
    <div className="wf-budget-patch">
      {field("turns", turns, setTurns)}
      {showTokens && field("tokens", tokens, setTokens)}
      {field("minutes", minutes, setMinutes)}
      <button
        type="button"
        className="btn-t primary"
        disabled={busy || !hasBump}
        onClick={() => void resume()}
      >
        <Icon name="play" size={13} /> Resume
      </button>
    </div>
  );
}
