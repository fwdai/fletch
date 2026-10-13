// The host's providers and their accounts, pushed from the Home header's
// settings button: for each provider that can hold several sign-ins, every
// account the Mac knows, which one its agents run under, and each account's
// plan limits — the desktop's Settings › Providers accounts list, read off the
// host and kept current by its `settings:changed` events. The one thing to do
// here is move the selection; adding and signing in stay on the Mac.

import { useEffect } from "react";
import { Nav } from "../../components/ui";
import { Notice } from "../../components/ui/Notice";
import { useTick } from "../../lib/hooks";
import { ignore } from "../../lib/ignore";
import { useStore } from "../../store";
import { ProviderSection } from "./ProviderSection";

/** How often the reset countdowns are redrawn. */
const CLOCK_MS = 30_000;

export function ProvidersScreen() {
  const hostName = useStore((s) => s.hostInfo?.name);
  const supported = useStore((s) => s.hostSupports("list_provider_accounts"));
  const accounts = useStore((s) => s.providerAccounts);
  const load = useStore((s) => s.providerAccountsLoad);
  const loadAccounts = useStore((s) => s.loadProviderAccounts);
  const pop = useStore((s) => s.pop);
  useTick(CLOCK_MS);

  // Read whole on every open: a sign-in done on the Mac between two visits has
  // no event, and the handshake carries none of this. While the screen is up,
  // the store reads again on every reconnect and return to the foreground
  // (store/index), since an event missed off-socket has no other way back.
  // The outcome lands in `providerAccountsLoad`, so a read that fails here and
  // succeeds there clears the message.
  useEffect(() => {
    if (supported) void loadAccounts().catch(ignore);
  }, [loadAccounts, supported]);

  const providers = Object.keys(accounts);
  const nowMs = Date.now();

  return (
    <>
      <Nav onBack={pop} backLabel="Home" title="Providers" sub={hostName} hair />
      <div className="scroll pv-body">
        <p className="pv-lead">
          Every agent runs under its provider's selected account: new ones from the start, running
          ones from their next turn. Accounts are added and signed in on the Mac.
        </p>
        {!supported && (
          <div className="empty">
            <b>Not available</b>
            This Mac's Fletch is too old to list its accounts.
          </div>
        )}
        {load?.status === "error" && (
          <Notice
            tone="error"
            action={{ label: "Retry", onClick: () => void loadAccounts().catch(ignore) }}
          >
            {load.error}
          </Notice>
        )}
        {supported && load?.status !== "error" && providers.length === 0 && (
          <div className="empty">Reading accounts from {hostName ?? "the Mac"}…</div>
        )}
        {providers.map((p) => (
          <ProviderSection key={p} providerId={p} accounts={accounts[p]} nowMs={nowMs} />
        ))}
      </div>
    </>
  );
}
