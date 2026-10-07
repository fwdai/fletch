import type { AccountLimits, ProviderAccount } from "@/api/types/providers";
import { Badge } from "@/components/ui/Badge";
import type { ProviderId } from "@/data/providers";
import { LimitMeter } from "./LimitMeter";
import { asOfLabel, refreshHint } from "./limitsFormat";

/** An account's plan limits under its row: the five-hour and weekly meters,
 *  how fresh the reading is and where it came from, and what the last Refresh
 *  ran into. With no reading yet it says what would get one. */
export function LimitsPanel({
  providerId,
  account,
  row,
  nowMs,
}: {
  providerId: ProviderId;
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
        ? "Sign in to see limits."
        : `No limits read yet. Refresh to ask ${providerId === "claude" ? "Claude" : "Codex"}.`);
    return <p className="set-prov-limits-empty text-xs">{text}</p>;
  }

  return (
    <div className="set-prov-limits">
      <div className="set-prov-limits-meters">
        <LimitMeter label="5-hour" window={limits.five_hour} nowMs={nowMs} />
        <LimitMeter label="Weekly" window={limits.seven_day} nowMs={nowMs} />
      </div>
      <div className="set-prov-limits-meta flex-center text-xs">
        <span>{asOfLabel(limits, nowMs)}</span>
        {hint && (
          <Badge variant="warn" hint={hint}>
            {row?.refresh?.status.replace("_", " ")}
          </Badge>
        )}
      </div>
    </div>
  );
}
