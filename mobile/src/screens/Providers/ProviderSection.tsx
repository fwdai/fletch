import type { ProviderAccount } from "@desktop/api/types/providers";
import { Icon } from "@desktop/components/Icon";
import { accountLabel } from "@desktop/data/providerAccounts";
import { useState } from "react";
import { ProviderMark } from "../../components/ui";
import { Notice } from "../../components/ui/Notice";
import { providerLabel } from "../../lib/agents";
import { useStore } from "../../store";
import { AccountCard } from "./AccountCard";

/** One provider's accounts: its mark and name, a Refresh that reads every
 *  account's limits again, and a card per account. */
export function ProviderSection({
  providerId,
  accounts,
  nowMs,
}: {
  providerId: string;
  accounts: ProviderAccount[];
  nowMs: number;
}) {
  const limits = useStore((s) => s.providerLimits[providerId]);
  const refreshLimits = useStore((s) => s.refreshProviderLimits);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Every account in turn, the default included — it is an account too. A
  // signed-out one is skipped: its limits can only be read with its own
  // login. The host holds back an account inside its refresh floor or a 429
  // back-off and answers with the stored row, so pressing again is harmless.
  const refreshAll = async () => {
    setRefreshing(true);
    setError(null);
    const errors: string[] = [];
    for (const account of accounts) {
      if (account.status === "signed_out") continue;
      try {
        await refreshLimits(providerId, account.id);
      } catch (e) {
        errors.push(`${accountLabel(account)}: ${e instanceof Error ? e.message : String(e)}`);
      }
    }
    setRefreshing(false);
    if (errors.length > 0) setError(errors.join(" "));
  };

  return (
    <section className="pv-provider">
      <div className="sect pv-provider-head">
        <ProviderMark id={providerId} lg />
        <span className="pv-provider-name">{providerLabel(providerId)}</span>
        <span className="grow" />
        <button
          type="button"
          className="sect-add"
          disabled={refreshing}
          onClick={() => void refreshAll()}
        >
          <Icon name="refresh" size={12} strokeWidth={2.2} />
          {refreshing ? "Refreshing…" : "Refresh limits"}
        </button>
      </div>
      {error && (
        <Notice tone="error" onDismiss={() => setError(null)}>
          {error}
        </Notice>
      )}
      <div className="card">
        {accounts.map((account) => (
          <AccountCard
            key={account.id}
            account={account}
            limits={limits?.[account.id]}
            nowMs={nowMs}
          />
        ))}
      </div>
    </section>
  );
}
