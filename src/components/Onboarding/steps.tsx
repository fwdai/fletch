// Onboarding welcome/auth screen + the brand mark. The requirement steps
// (git / GitHub / agents / ready) live in their own files; the shared
// two-column frame is SetupBits' SetupStep.

import { open as openExternal } from "@tauri-apps/plugin-shell";
import type { CSSProperties } from "react";
import { Icon } from "@/components/Icon";

const TERMS_URL = "https://fletch.sh/terms";
const PRIVACY_URL = "https://fletch.sh/privacy";

// ── brand mark: the Fletch triple-peak ──────────────────────────────
export function PeaksMark() {
  return (
    <svg
      viewBox="0 0 48 48"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M5 36 L 15 20 L 21 27 L 30 9 L 37 22 L 41 17 L 44 24" />
      <line x1="3" y1="40" x2="45" y2="40" strokeWidth="1" opacity=".4" />
      <circle cx="30" cy="9" r="2.4" fill="currentColor" stroke="none" />
    </svg>
  );
}

// ── Google "G" (multi-color, standard OAuth mark) ───────────────────
function GoogleG() {
  return (
    <svg viewBox="0 0 18 18" width="20" height="20" aria-hidden="true">
      <path
        fill="#4285F4"
        d="M17.64 9.2c0-.64-.06-1.25-.16-1.84H9v3.48h4.84a4.14 4.14 0 0 1-1.8 2.72v2.26h2.92c1.71-1.57 2.68-3.89 2.68-6.62z"
      />
      <path
        fill="#34A853"
        d="M9 18c2.43 0 4.47-.8 5.96-2.18l-2.92-2.26c-.81.54-1.84.86-3.04.86-2.34 0-4.32-1.58-5.03-3.7H.96v2.33A9 9 0 0 0 9 18z"
      />
      <path
        fill="#FBBC05"
        d="M3.97 10.72a5.4 5.4 0 0 1 0-3.44V4.95H.96a9 9 0 0 0 0 8.1l3.01-2.33z"
      />
      <path
        fill="#EA4335"
        d="M9 3.58c1.32 0 2.5.45 3.44 1.35l2.58-2.58A9 9 0 0 0 .96 4.95l3.01 2.33C4.68 5.16 6.66 3.58 9 3.58z"
      />
    </svg>
  );
}

// ── Step · Welcome + auth ───────────────────────────────────────────
export function WelcomeStep({
  onAuth,
  busy,
}: {
  onAuth: (provider: string) => void;
  busy: string | null;
}) {
  return (
    <div className="ob-step">
      <div className="ob-welcome">
        <div className="ob-brand ob-reveal" style={{ "--d": ".05s" } as CSSProperties}>
          <span className="mk">
            <PeaksMark />
          </span>
          <span className="wd text-lg">FLETCH</span>
        </div>
        <h1 className="ob-display ob-reveal" style={{ "--d": ".16s" } as CSSProperties}>
          A new era of <em>agentic</em> engineering.
        </h1>
        <p className="ob-lede ob-reveal" style={{ "--d": ".30s" } as CSSProperties}>
          Direct a fleet of coding agents in parallel — each in its own checkout. Review, refine,
          and ship from one quiet control room.
        </p>

        <div className="ob-auth ob-reveal" style={{ "--d": ".44s" } as CSSProperties}>
          <button
            className={`ob-authbtn primary ${busy === "github" ? "busy" : ""}`}
            onClick={() => onAuth("github")}
          >
            <span className="gl">
              <Icon name="github" size={20} />
            </span>
            <span className="lbl">Continue with GitHub</span>
            <span className="ent">↵</span>
          </button>
          <button
            className={`ob-authbtn ${busy === "google" ? "busy" : ""}`}
            onClick={() => onAuth("google")}
          >
            <span className="gl">
              <GoogleG />
            </span>
            <span className="lbl">Continue with Google</span>
          </button>
        </div>

        <p className="ob-legal ob-reveal text-xs" style={{ "--d": ".58s" } as CSSProperties}>
          By continuing you agree to Fletch's{" "}
          <a
            href={TERMS_URL}
            onClick={(e) => {
              e.preventDefault();
              void openExternal(TERMS_URL).catch(() => {});
            }}
          >
            Terms
          </a>{" "}
          and{" "}
          <a
            href={PRIVACY_URL}
            onClick={(e) => {
              e.preventDefault();
              void openExternal(PRIVACY_URL).catch(() => {});
            }}
          >
            Privacy Policy
          </a>
          . Your code never leaves your machine.
        </p>
      </div>
    </div>
  );
}
