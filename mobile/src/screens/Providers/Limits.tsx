import type { AccountLimits, ProviderAccount } from "@desktop/api/types/providers";
import {
  asOfLabel,
  olderModelsLabel,
  refreshHint,
} from "@desktop/components/SettingsScreen/ProvidersPane/AccountsSection/limitsFormat";
import { providerLabel } from "../../lib/agents";
import { LimitMeter } from "./LimitMeter";

/** An account's plan limits under its row — the desktop's LimitsPanel: the
 *  five-hour and weekly meters, one weekly meter per model the source scoped a
 *  window to, how fresh the reading is and where it came from, and what the
 *  last Refresh ran into. With no reading yet it says what would get one. */
export function Limits({
  account,
  row,
  nowMs,
}: {
  account: ProviderAccount;
  row: AccountLimits | undefined;
  nowMs: number;
}) {
  const limits = row?.limits ?? null;
  const hint = refreshHint(row?.refresh ?? null, nowMs);

  if (!limits) {
    // Limits are read with the account's own login, so a signed-out account
    // can't have any — and Refresh skips it — so say what would.
    const text =
      hint ??
      (account.status === "signed_out"
        ? "Sign in on the Mac to see limits."
        : `No limits read yet. Refresh to ask ${providerLabel(account.provider)}.`);
    return <p className="pv-limits-empty">{text}</p>;
  }

  const olderModels = olderModelsLabel(limits, nowMs);
  return (
    <div className="pv-limits">
      <div className="pv-limits-meters">
        <LimitMeter label="5-hour" window={limits.five_hour} nowMs={nowMs} />
        <LimitMeter label="Weekly" window={limits.seven_day} nowMs={nowMs} />
        {limits.models?.map((m) => (
          <LimitMeter key={m.model} label={`Weekly · ${m.model}`} window={m} nowMs={nowMs} />
        ))}
      </div>
      <div className="pv-limits-meta">
        {asOfLabel(limits, nowMs)}
        {olderModels && ` · ${olderModels}`}
        {hint && <span className="pv-limits-hint">{hint}</span>}
      </div>
    </div>
  );
}
