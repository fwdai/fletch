// The accounts half of a provider that can hold several sign-ins (claude,
// codex — `supportsAccounts`): every account Fletch knows for it, which one
// its agents run under (every agent, from its next turn — the host moves them),
// a Sign in per account, its plan limits, and a way to add one. Takes the
// place of SignInSection for those providers; the single-login providers keep
// SignInSection.
//
// One sign-in terminal at a time, like SignInSection: the section tracks
// which account's is open, so two logins can't race each other for the
// browser.

import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/Button";
import type { ProviderId } from "@/data/providers";
import { useAppStore } from "@/store";
import { useLimitsRefresh } from "@/util/useLimitsRefresh";
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
  const [signingIn, setSigningIn] = useState<string | null>(null);
  const { refreshing, error: limitsError, refresh, refreshOnOpen } = useLimitsRefresh();
  const nowMs = useNow();

  // Asked once, when the section first has accounts and their stored rows.
  const askedOnOpen = useRef(false);
  const ready = accounts !== undefined && limits !== undefined;
  useEffect(() => {
    if (!ready || askedOnOpen.current) return;
    askedOnOpen.current = true;
    void refreshOnOpen([providerId]);
  }, [providerId, ready, refreshOnOpen]);

  // Not listed yet (first probe in flight): nothing to show rather than an
  // empty section that then jumps.
  if (!accounts) return null;

  return (
    <div className="set-prov-accounts">
      <div className="set-prov-acct-head text-sm">
        <span className="set-prov-dk">Accounts</span>
        <span>New agents use the selected account; running ones switch on their next turn.</span>
        <Button
          variant="outline"
          size="sm"
          disabled={refreshing}
          onClick={() => void refresh([providerId])}
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
          refreshingAll={refreshing}
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
