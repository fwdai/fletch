import { useCallback, useEffect, useRef } from "react";
import { useAgentAvailability } from "@/components/Composer/availability";
import { Icon } from "@/components/Icon";
import { useNow } from "@/components/SettingsScreen/ProvidersPane/AccountsSection/useNow";
import { IconButton } from "@/components/ui/IconButton";
import { APP_POPOVER_SCRIM_Z, Scrim } from "@/components/ui/Scrim";
import { Spinner } from "@/components/ui/Spinner";
import { useFocusScope } from "@/components/ui/useFocusScope";
import { supportsAccounts } from "@/data/providerDetail";
import { accountLimits, limitsRefreshKey } from "@/data/providerLimits";
import { PROVIDERS } from "@/data/providers";
import { useAppStore } from "@/store";
import { useLimitsRefresh } from "@/util/useLimitsRefresh";
import { AccountBlock } from "./AccountBlock";
import { UsageReportRow } from "./UsageReportRow";

export { LimitsButton } from "./LimitsButton";

/** Every account's plan limits at a glance: one click (or ⌘U) from anywhere,
 *  where Settings › Providers is a screen, a row and an expand away. It reads
 *  every account afresh as it opens, so what it shows is current. Mounted
 *  at app root and pinned bottom-left, above the sidebar footer's gauge that
 *  opens it, so the chord works with the sidebar hidden too. An outside click,
 *  Escape or any dialog opened after it dismisses it. */
export function LimitsPopover() {
  const open = useAppStore((s) => s.limitsPopoverOpen);
  const toggle = useAppStore((s) => s.toggleLimitsPopover);
  const close = useCallback(() => toggle(false), [toggle]);
  if (!open) return null;
  return (
    <>
      <Scrim onClose={close} zIndex={APP_POPOVER_SCRIM_Z} />
      <LimitsCard onGiveWay={close} />
    </>
  );
}

function LimitsCard({ onGiveWay }: { onGiveWay: () => void }) {
  const providerAccounts = useAppStore((s) => s.providerAccounts);
  const providerLimits = useAppStore((s) => s.providerLimits);
  const inFlight = useAppStore((s) => s.providerLimitsRefreshing);
  const providerFlags = useAppStore((s) => s.providerFlags);
  const refreshAccounts = useAppStore((s) => s.refreshProviderAccounts);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);
  // Accounts and their limits are this Mac's, whichever host is active.
  const availability = useAgentAvailability("local");
  const { refreshing, error, refresh } = useLimitsRefresh();
  const nowMs = useNow();
  const cardRef = useRef<HTMLDivElement>(null);
  useFocusScope(cardRef, onGiveWay);

  // The providers that keep accounts, installed here and switched on, each
  // with the accounts Settings lists for it.
  const listed = PROVIDERS.filter(
    (p) => supportsAccounts(p.id) && availability(p.id).installed && providerFlags[p.id] !== false,
  )
    .map((p) => ({ id: p.id, accounts: providerAccounts[p.id] ?? [] }))
    .filter((p) => p.accounts.length > 0);
  const ids = listed.map((p) => p.id);

  // The card opens on the stored readings and asks for fresh ones behind
  // them: the accounts are listed again (they are whatever boot or the last
  // Settings visit left), then every signed-in one is read. Opening it again
  // within the engine's refresh floor costs the vendor nothing.
  const idsRef = useRef(ids);
  idsRef.current = ids;
  useEffect(() => {
    void refreshAccounts().then(() => refresh(idsRef.current));
  }, [refreshAccounts, refresh]);

  return (
    <div ref={cardRef} className="lim-pop" role="dialog" aria-label="Plan limits">
      <div className="lim-pop-head flex-center">
        <span className="lim-pop-title text-base">Plan limits</span>
        <IconButton size="sm" tip="Manage accounts" onClick={() => openSettingsScreen("providers")}>
          <Icon name="settings" />
        </IconButton>
        <IconButton
          size="sm"
          tip="Refresh limits"
          disabled={refreshing || ids.length === 0}
          onClick={() => void refresh(ids)}
        >
          {refreshing ? <Spinner size={12} /> : <Icon name="refresh" />}
        </IconButton>
      </div>
      {error && <p className="lim-pop-error text-xs">{error}</p>}
      {listed.length > 0 ? (
        <div className="lim-pop-list">
          {listed.flatMap((p) =>
            p.accounts.map((account) => (
              <AccountBlock
                key={`${p.id}/${account.id}`}
                providerId={p.id}
                account={account}
                solo={p.accounts.length === 1}
                row={accountLimits(providerLimits[p.id], account)}
                refreshing={limitsRefreshKey(p.id, account.id) in inFlight}
                nowMs={nowMs}
              />
            )),
          )}
        </div>
      ) : (
        <p className="lim-pop-empty text-sm">
          No plan limits to show yet. Claude Code and Codex report theirs once they are installed
          and signed in.
        </p>
      )}
      <UsageReportRow />
    </div>
  );
}
