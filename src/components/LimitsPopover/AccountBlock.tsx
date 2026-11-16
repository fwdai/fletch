import type { AccountLimits, ProviderAccount } from "@/api/types/providers";
import { ProviderIcon } from "@/components/ProviderIcon";
import { LimitsPanel } from "@/components/SettingsScreen/ProvidersPane/AccountsSection/LimitsPanel";
import { agoLabel } from "@/components/SettingsScreen/ProvidersPane/AccountsSection/limitsFormat";
import { Badge } from "@/components/ui/Badge";
import { accountLabel } from "@/data/providerAccounts";
import { type ProviderId, providerChip, providerLabel } from "@/data/providers";

/** One account in the popover: whose it is and how old its reading is, over
 *  the same limits panel Settings › Providers shows, in its compact form.
 *  `solo` is a provider's only account, which the provider's name alone
 *  already identifies. */
export function AccountBlock({
  providerId,
  account,
  solo,
  row,
  nowMs,
}: {
  providerId: ProviderId;
  account: ProviderAccount;
  solo: boolean;
  row: AccountLimits | undefined;
  nowMs: number;
}) {
  const asOf = row?.limits?.as_of;
  return (
    <section className="lim-acct">
      <div className="lim-acct-head flex-center text-sm">
        <ProviderIcon slug={providerId} {...providerChip(providerId)} size={16} />
        <span className="lim-acct-title truncate">{providerLabel(providerId)}</span>
        {!solo && <span className="lim-acct-name truncate">{accountLabel(account)}</span>}
        {!solo && account.active && (
          <Badge variant="new" tip="New agents run under this account">
            active
          </Badge>
        )}
        {asOf !== undefined && (
          <span className="lim-acct-age text-xs">{agoLabel(asOf, nowMs)}</span>
        )}
      </div>
      <LimitsPanel providerId={providerId} account={account} row={row} nowMs={nowMs} compact />
    </section>
  );
}
