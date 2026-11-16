import { Icon } from "@/components/Icon";
import { limitTone } from "@/components/SettingsScreen/ProvidersPane/AccountsSection/limitsFormat";
import { useNow } from "@/components/SettingsScreen/ProvidersPane/AccountsSection/useNow";
import { Chip } from "@/components/ui/Chip";
import { activeAccountId } from "@/data/providerAccounts";
import { tightestWindow } from "@/data/providerLimits";
import { providerLabel } from "@/data/providers";
import { useAppStore } from "@/store";
import { useShortcutKeys } from "@/util/shortcuts";
import { useListedAccounts } from "./useListedAccounts";

/** The sidebar footer's way into the usage popover: a chip carrying the share
 *  used of the plan window closest to its limit, among the accounts each
 *  provider's new agents run under — the one that will hold an agent up
 *  first, whichever provider it is. The tip names it, and the figure takes the
 *  meters' tint past their warning line. With no reading still holding, the
 *  chip carries no figure. */
export function LimitsButton() {
  const open = useAppStore((s) => s.limitsPopoverOpen);
  const toggle = useAppStore((s) => s.toggleLimitsPopover);
  const providerLimits = useAppStore((s) => s.providerLimits);
  const keys = useShortcutKeys("limits");
  const listed = useListedAccounts();
  const nowMs = useNow(60_000);

  const tightest = tightestWindow(
    listed.map((p) => ({
      provider: p.id,
      row: providerLimits[p.id]?.[activeAccountId(p.accounts)],
    })),
    nowMs,
  );
  const percent = tightest ? Math.round(tightest.window.percent) : null;
  const usage = keys ? `Usage (${keys})` : "Usage";
  const tip = tightest
    ? `${usage} · ${providerLabel(tightest.provider)} ${tightest.name} at ${percent}%`
    : usage;

  return (
    <Chip
      bordered
      className={`lim-chip ${limitTone(percent)}${open ? " active" : ""}`}
      tip={tip}
      aria-label="Usage"
      aria-expanded={open}
      onClick={() => toggle()}
    >
      <Icon name="activity" />
      {percent !== null && <span className="lim-chip-fig mono">{percent}%</span>}
      <Icon name="chevU" className="lim-chip-caret" />
    </Chip>
  );
}
