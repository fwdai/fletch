// The host's providers and their accounts, pushed from the Home header's
// settings button: for each provider that can hold several sign-ins, every
// account the Mac knows, which one its agents run under, and each account's
// plan limits — the desktop's Settings › Providers accounts list, read off the
// host and kept current by its `settings:changed` events. The one thing to do
// here is move the selection; adding and signing in stay on the Mac.

import { useEffect, useState } from "react";
import { Nav } from "../../components/ui";
import { Notice } from "../../components/ui/Notice";
import { useTick } from "../../lib/hooks";
import { useStore } from "../../store";
import { ProviderSection } from "./ProviderSection";

/** How often the reset countdowns are redrawn. */
const CLOCK_MS = 30_000;

export function ProvidersScreen() {
  const hostName = useStore((s) => s.hostInfo?.name);
  const supported = useStore((s) => s.hostSupports("list_provider_accounts"));
  const accounts = useStore((s) => s.providerAccounts);
  const load = useStore((s) => s.loadProviderAccounts);
  const pop = useStore((s) => s.pop);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useTick(CLOCK_MS);

  // Read whole on every open: a sign-in done on the Mac between two visits has
  // no event, and the handshake carries none of this.
  useEffect(() => {
    if (!supported) return;
    let live = true;
    load()
      .then(() => {
        if (live) setLoaded(true);
      })
      .catch((e) => {
        if (live) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      live = false;
    };
  }, [load, supported]);

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
        {error && <Notice tone="error">{error}</Notice>}
        {supported && !loaded && !error && providers.length === 0 && (
          <div className="empty">Reading accounts from {hostName ?? "the Mac"}…</div>
        )}
        {providers.map((p) => (
          <ProviderSection key={p} providerId={p} accounts={accounts[p]} nowMs={nowMs} />
        ))}
      </div>
    </>
  );
}
