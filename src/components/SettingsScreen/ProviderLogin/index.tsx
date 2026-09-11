import { useState } from "react";
import { Button } from "@/components/ui/Button";
import { loginCommand, PROVIDER_DETAIL } from "@/data/providerDetail";
import type { ProviderId } from "@/data/providers";
import { useProviderLogin } from "./useProviderLogin";

interface Props {
  providerId: ProviderId;
  /** Product name, for the terminal's label ("Signing in to Codex CLI"). */
  providerLabel: string;
  /** Dismiss the terminal. Called once the sign-in PTY has actually been
   *  killed (the Close button awaits that), never before. */
  onClose: () => void;
}

/** An embedded terminal running an agent CLI's own sign-in command, so the user
 *  completes auth without leaving Settings. Every CLI does this differently
 *  (browser OAuth, a device code, an interactive provider picker) — a real PTY
 *  is the one surface that works for all of them.
 *
 *  Only render this for a provider that has a `loginCommand`; antigravity and
 *  pi have none and show their `signIn` hint alone. */
export function ProviderLoginTerminal({ providerId, providerLabel, onClose }: Props) {
  const { containerRef, exit, runAgain, close } = useProviderLogin(providerId);
  const command = loginCommand(providerId);
  const hint = PROVIDER_DETAIL[providerId].signIn;
  // Close is awaited before the row collapses: dismissing first would let a
  // quick re-open attach to the PTY this close is still about to kill. The
  // controls lock meanwhile so nothing else can act on the dying session.
  const [closing, setClosing] = useState(false);

  const closeAndDismiss = async () => {
    if (closing) return;
    setClosing(true);
    try {
      await close();
    } finally {
      onClose();
    }
  };

  return (
    <div className="prov-login">
      <div className="prov-login-head flex-center">
        <code className="prov-login-cmd">$ {command}</code>
        {hint && <span className="prov-login-hint text-sm">{hint}</span>}
        <Button variant="ghost" size="sm" disabled={closing} onClick={() => void closeAndDismiss()}>
          {closing ? "Closing…" : "Close"}
        </Button>
      </div>

      <div className="prov-login-term xterm-slot">
        <div
          ref={containerRef}
          className="xterm-host"
          style={{ inset: "8px 4px 8px 10px" }}
          aria-label={`Signing in to ${providerLabel}`}
        />
      </div>

      {exit && (
        <div className="prov-login-foot flex-center">
          <span className={`prov-login-exit text-sm ${exit.success ? "ok" : "bad"}`}>
            {exit.success ? "Finished" : exit.message}
          </span>
          <Button variant="outline" size="sm" disabled={closing} onClick={runAgain}>
            Run again
          </Button>
        </div>
      )}
    </div>
  );
}
