// The accounts half of a provider that can hold several sign-ins (claude,
// codex — `supportsAccounts`): every account Fletch knows for it, which one
// new agents use, a Sign in per account, its plan limits, and a way to add
// one. Takes the place of SignInSection for those providers; the single-login
// providers keep SignInSection.
//
// One sign-in terminal at a time, like SignInSection: the section tracks
// which account's is open, so two logins can't race each other for the
// browser.

import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/Button";
import { accountLabel } from "@/data/providerAccounts";
import type { ProviderId } from "@/data/providers";
import { useAppStore } from "@/store";
import { AccountRow } from "./AccountRow";
import { AddAccountForm } from "./AddAccountForm";
import { useNow } from "./useNow";

export function AccountsSection({
  providerId,
  providerLabel,
}: {
  providerId: ProviderId;
  providerLabel: string;
}) {
  const accounts = useAppStore((s) => s.providerAccounts[providerId]);
  const limits = useAppStore((s) => s.providerLimits[providerId]);
  const refreshLimits = useAppStore((s) => s.refreshProviderLimits);
  const [signingIn, setSigningIn] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [limitsError, setLimitsError] = useState<string | null>(null);
  const nowMs = useNow();

  // Every account in turn, the default included — it is an account too. A
  // signed-out one is skipped: its limits can only be read with its own login.
  // The engine holds back an account inside its refresh floor or a 429
  // back-off and answers with the stored row, so pressing again is harmless.
  const refreshAll = useCallback(async () => {
    const list = useAppStore.getState().providerAccounts[providerId] ?? [];
    setRefreshing(true);
    setLimitsError(null);
    const errors: string[] = [];
    for (const account of list) {
      if (account.status === "signed_out") continue;
      try {
        await refreshLimits(providerId, account.id);
      } catch (err) {
        errors.push(`${accountLabel(account)}: ${String(err)}`);
      }
    }
    setRefreshing(false);
    if (errors.length > 0) setLimitsError(errors.join(" "));
  }, [providerId, refreshLimits]);

  // Codex answers from the app-server without spending quota, so its limits
  // are asked for once when the section first has accounts to ask about.
  // Claude's endpoint rate-limits hard: only the button asks it.
  const askedOnOpen = useRef(false);
  const listed = accounts !== undefined;
  useEffect(() => {
    if (providerId !== "codex" || !listed || askedOnOpen.current) return;
    askedOnOpen.current = true;
    void refreshAll();
  }, [providerId, listed, refreshAll]);

  // Not listed yet (first probe in flight): nothing to show rather than an
  // empty section that then jumps.
  if (!accounts) return null;

  return (
    <div className="set-prov-accounts">
      <div className="set-prov-acct-head text-sm">
        <span className="set-prov-dk">Accounts</span>
        <span>
          New agents use the selected account. Each account keeps its own login and sessions;
          settings, commands and skills are shared from your terminal setup.
        </span>
        <Button
          variant="outline"
          size="sm"
          disabled={refreshing}
          onClick={() => void refreshAll()}
          tip={`Read each account's ${providerLabel} limits now`}
        >
          {refreshing ? "Refreshing…" : "Refresh limits"}
        </Button>
      </div>
      {limitsError && <p className="set-prov-acct-error text-sm">{limitsError}</p>}
      {accounts.map((account) => (
        <AccountRow
          key={account.id}
          account={account}
          providerId={providerId}
          providerLabel={providerLabel}
          signingIn={signingIn === account.id}
          onSignIn={() => setSigningIn(account.id)}
          onCloseSignIn={() => setSigningIn(null)}
          limits={limits?.[account.id]}
          nowMs={nowMs}
        />
      ))}
      <AddAccountForm providerId={providerId} />
    </div>
  );
}
