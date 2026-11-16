import type { AccountLimits, ProviderAccount } from "@/api/types/providers";
import { Badge } from "@/components/ui/Badge";
import type { ProviderId } from "@/data/providers";
import { LimitMeter } from "./LimitMeter";
import { agoLabel, refreshHint } from "./limitsFormat";

/** An account's plan limits under its row, in two columns: the five-hour
 *  meter on the left; on the right the weekly meter, then one weekly meter
 *  per model the source scoped a window to ("Fable"). A model window older
 *  than the reading it rides on says so on its own reset line. Under them,
 *  what the last Refresh ran into, if anything. With no reading yet it says
 *  what would get one.
 *
 *  `compact` is the same panel as a single column of one-line meters, their
 *  names shortened to fit ("5h", "7d", "7d Fable"). */
export function LimitsPanel({
  providerId,
  account,
  row,
  nowMs,
  compact,
}: {
  providerId: ProviderId;
  account: ProviderAccount;
  row: AccountLimits | undefined;
  nowMs: number;
  compact?: boolean;
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
    <div className={`set-prov-limits${compact ? " compact" : ""}`}>
      <div className="set-prov-limits-meters">
        <LimitMeter
          label={compact ? "5h" : "5-hour"}
          window={limits.five_hour}
          nowMs={nowMs}
          compact={compact}
        />
        <div className="set-prov-limits-weekly">
          <LimitMeter
            label={compact ? "7d" : "Weekly"}
            window={limits.seven_day}
            nowMs={nowMs}
            compact={compact}
          />
          {limits.models?.map((m) => (
            <LimitMeter
              key={m.model}
              label={compact ? `7d ${m.model}` : `Weekly · ${m.model}`}
              window={m}
              nowMs={nowMs}
              note={m.as_of < limits.as_of ? `as of ${agoLabel(m.as_of, nowMs)}` : null}
              compact={compact}
            />
          ))}
        </div>
      </div>
      {hint && (
        <div className="set-prov-limits-meta flex-center text-xs">
          <Badge variant="warn" hint={hint}>
            {row?.refresh?.status.replace("_", " ")}
          </Badge>
        </div>
      )}
    </div>
  );
}
