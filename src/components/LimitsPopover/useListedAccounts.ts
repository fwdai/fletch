import type { ProviderAccount } from "@/api/types/providers";
import { useAgentAvailability } from "@/components/Composer/availability";
import { supportsAccounts } from "@/data/providerDetail";
import { PROVIDERS, type ProviderId } from "@/data/providers";
import { useAppStore } from "@/store";

/** The providers whose plan limits Usage shows: those that keep accounts,
 *  installed on this Mac and switched on, each with the accounts Settings
 *  lists for it. A provider with no account listed yet is left out. Accounts
 *  and their limits are this Mac's, whichever host is active. */
export function useListedAccounts(): { id: ProviderId; accounts: ProviderAccount[] }[] {
  const providerAccounts = useAppStore((s) => s.providerAccounts);
  const providerFlags = useAppStore((s) => s.providerFlags);
  const availability = useAgentAvailability("local");
  return PROVIDERS.filter(
    (p) => supportsAccounts(p.id) && availability(p.id).installed && providerFlags[p.id] !== false,
  )
    .map((p) => ({ id: p.id, accounts: providerAccounts[p.id] ?? [] }))
    .filter((p) => p.accounts.length > 0);
}
