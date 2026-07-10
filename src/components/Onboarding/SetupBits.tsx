// Shared atoms for the functional onboarding steps: the editorial left
// column (eyebrow / title / lede / points — the Beat layout, kept from the
// tour), plus the small copy-command and docs-link affordances the status
// cards use.

import { open as openExternal } from "@tauri-apps/plugin-shell";
import { type CSSProperties, type ReactNode, useState } from "react";
import { Icon, type IconName } from "@/components/Icon";

export interface StepPoint {
  icon: IconName;
  head: string;
  body: string;
}

/** Two-column step frame: editorial copy + status card on the left (children
 *  render below the points — that's where each step's functional card goes),
 *  exhibit on the right. */
export function SetupStep({
  num,
  eyebrow,
  title,
  lede,
  points,
  exhibit,
  children,
}: {
  num: string;
  eyebrow: string;
  title: ReactNode;
  lede: ReactNode;
  points: StepPoint[];
  exhibit: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="ob-step">
      <div className="ob-beat">
        <div className="ob-beat-copy">
          <div className="ob-eyebrow ob-reveal text-xs" style={{ "--d": ".05s" } as CSSProperties}>
            <span className="num">{num}</span>
            <span className="ln" />
            <span>{eyebrow}</span>
          </div>
          <h2 className="ob-display ob-reveal" style={{ "--d": ".14s" } as CSSProperties}>
            {title}
          </h2>
          <p className="ob-lede ob-reveal" style={{ "--d": ".24s" } as CSSProperties}>
            {lede}
          </p>
          <div className="ob-points">
            {points.map((p, i) => (
              <div
                key={p.head}
                className="ob-point ob-reveal"
                style={{ "--d": `${0.32 + i * 0.07}s` } as CSSProperties}
              >
                <span className="ic">
                  <Icon name={p.icon} size={12} />
                </span>
                <span>
                  <b>{p.head}</b> {p.body}
                </span>
              </div>
            ))}
          </div>
          {children}
        </div>
        {exhibit}
      </div>
    </div>
  );
}

export function CopyCmd({ cmd }: { cmd: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className="rdy-cmd iflex-center"
      title="Copy command"
      onClick={() => {
        void navigator.clipboard.writeText(cmd);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      }}
    >
      <code>{cmd}</code>
      <Icon name={copied ? "check" : "copy"} size={10} />
    </button>
  );
}

export function DocsLink({ url, label = "Setup guide" }: { url: string; label?: string }) {
  return (
    <button
      type="button"
      className="rdy-docs iflex-center text-sm"
      onClick={() => void openExternal(url)}
    >
      {label}
      <Icon name="external" size={10} />
    </button>
  );
}
