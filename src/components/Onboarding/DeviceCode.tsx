// Shown during an OAuth device-flow login: the user code to enter at the
// provider's verification page (the browser is opened automatically), plus
// the starting / error states. Purely presentational — the parent owns the
// flow and the cancel behaviour.
import type { CSSProperties } from "react";

export interface DeviceCodeInfo {
  provider: string;
  userCode: string;
  verificationUri: string;
}

export function DeviceCode({
  info,
  error,
  onCancel,
}: {
  info: DeviceCodeInfo | null;
  error: string | null;
  onCancel: () => void;
}) {
  const label = (info?.provider ?? "") === "google" ? "Google" : "GitHub";
  return (
    <div className="ob-step">
      <div className="ob-device ob-reveal" style={{ "--d": ".1s" } as CSSProperties}>
        {error ? (
          <>
            <p className="ob-device-err">{error}</p>
            <button className="ob-authbtn" onClick={onCancel}>
              Back
            </button>
          </>
        ) : info ? (
          <>
            <p className="ob-device-lede">
              Finish signing in to {label} in your browser, then enter this code:
            </p>
            <div className="ob-device-code text-4xl">{info.userCode}</div>
            <p className="ob-device-uri text-base">{info.verificationUri}</p>
            <button className="ob-cancel" onClick={onCancel}>
              Cancel
            </button>
          </>
        ) : (
          <p className="ob-device-lede">Starting {label} sign-in…</p>
        )}
      </div>
    </div>
  );
}
