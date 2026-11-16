import { Icon } from "@/components/Icon";
import { limitTone } from "@/components/SettingsScreen/ProvidersPane/AccountsSection/limitsFormat";
import { useNow } from "@/components/SettingsScreen/ProvidersPane/AccountsSection/useNow";
import { Chip } from "@/components/ui/Chip";
import { weeklyUsage } from "@/data/providerLimits";
import { useAppStore } from "@/store";
import { useShortcutKeys } from "@/util/shortcuts";
import { useListedAccounts } from "./useListedAccounts";

/** The sidebar footer's way into the usage popover: a chip carrying how much
 *  of the weekly allowance is used across the signed-in accounts the popover
 *  lists (see `weeklyUsage`). The tip says what the figure covers, and the figure
 *  takes the meters' tint past their warning line. With no weekly reading, the
 *  chip carries no figure. */
export function LimitsButton() {
  const open = useAppStore((s) => s.limitsPopoverOpen);
  const toggle = useAppStore((s) => s.toggleLimitsPopover);
  const providerLimits = useAppStore((s) => s.providerLimits);
  const keys = useShortcutKeys("limits");
  const listed = useListedAccounts();
  const nowMs = useNow(60_000);

  // A signed-out account's allowance can't be used, and its reading is no
  // longer refreshed, so it stays out of the figure.
  const weekly = weeklyUsage(
    listed.flatMap((p) =>
      p.accounts.filter((a) => a.status !== "signed_out").map((a) => providerLimits[p.id]?.[a.id]),
    ),
    nowMs,
  );
  const percent = weekly ? Math.round(weekly.percent) : null;
  const detail = weekly
    ? `${percent}% of the weekly allowance used across ${weekly.accounts} ${
        weekly.accounts === 1 ? "account" : "accounts"
      }`
    : null;
  const usage = keys ? `Usage (${keys})` : "Usage";

  return (
    <Chip
      bordered
      className={`lim-chip ${limitTone(percent)}${open ? " active" : ""}`}
      tip={detail ? `${usage} · ${detail}` : usage}
      // The label replaces the chip's text as its accessible name, so it
      // carries the figure the chip shows.
      aria-label={detail ? `Usage · ${detail}` : "Usage"}
      aria-expanded={open}
      onClick={() => toggle()}
    >
      <Icon name="activity" />
      {percent !== null && <span className="lim-chip-fig mono">{percent}%</span>}
      <Icon name="chevU" className="lim-chip-caret" />
    </Chip>
  );
}
