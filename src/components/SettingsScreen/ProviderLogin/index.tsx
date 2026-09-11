import { Button } from "@/components/ui/Button";
import { loginCommand, PROVIDER_DETAIL } from "@/data/providerDetail";
import type { ProviderId } from "@/data/providers";
import { useProviderLogin } from "./useProviderLogin";

interface Props {
  providerId: ProviderId;
  /** Product name, for the terminal's label ("Signing in to Codex CLI"). */
  providerLabel: string;
  /** Dismiss the terminal. Called after the sign-in PTY has been killed. */
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

  return (
    <div className="prov-login">
      <div className="prov-login-head flex-center">
        <code className="prov-login-cmd">$ {command}</code>
        {hint && <span className="prov-login-hint text-sm">{hint}</span>}
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            close();
            onClose();
          }}
        >
          Close
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
          <Button variant="outline" size="sm" onClick={runAgain}>
            Run again
          </Button>
        </div>
      )}
    </div>
  );
}
