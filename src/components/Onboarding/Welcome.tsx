// Screen 1 · Welcome. A splash, not a form: the lockup, the line, one button.
// It carries the legal and data-sharing disclosure, which is why the first
// `app_opened` is deferred to this screen's mount (see index.tsx). Nothing
// here needs an account — Fletch runs locally.

import { open as openExternal } from "@tauri-apps/plugin-shell";
import type { CSSProperties } from "react";
import { FletchLockup } from "@/components/FletchLockup";

const TERMS_URL = "https://fletch.sh/terms";
const PRIVACY_URL = "https://fletch.sh/privacy";

function ExternalLink({ href, children }: { href: string; children: string }) {
  return (
    <a
      href={href}
      onClick={(e) => {
        e.preventDefault();
        void openExternal(href).catch(() => {});
      }}
    >
      {children}
    </a>
  );
}

export function Welcome({ onStart }: { onStart: () => void }) {
  return (
    <div className="ob-step ob-welcome-step">
      <div className="ob-welcome">
        <FletchLockup className="ob-hero ob-reveal" />
        <h1 className="ob-display ob-reveal" style={{ "--d": ".16s" } as CSSProperties}>
          The IDE for
          <br />
          <em>agentic</em> engineering.
        </h1>
        <p className="ob-lede ob-reveal" style={{ "--d": ".28s" } as CSSProperties}>
          Agents run in parallel in sealed sandboxes, navigate your codebase through a code graph,
          and follow workflows you define. They do the coding. You own what ships.
        </p>
        <div className="ob-start ob-reveal" style={{ "--d": ".4s" } as CSSProperties}>
          <button type="button" className="ob-cta lg" onClick={onStart}>
            Get started
            <span className="ent">↵</span>
          </button>
        </div>
      </div>
      {/* fine print lives at the foot of the stage, out of the hero's cluster */}
      <p className="ob-legal ob-reveal text-xs" style={{ "--d": ".54s" } as CSSProperties}>
        By continuing you agree to the <ExternalLink href={TERMS_URL}>Terms</ExternalLink> and{" "}
        <ExternalLink href={PRIVACY_URL}>Privacy Policy</ExternalLink>. Your code never leaves your
        machine.
        <br />
        Anonymous usage stats can be turned off in Settings › General.
      </p>
    </div>
  );
}
