import { open } from "@tauri-apps/plugin-shell";
import type { ReactNode } from "react";
import { Icon } from "@/components/Icon";

/** A small CSS spinner used in loading states (status line + busy CTA). */
export function Spinner() {
  return <span className="git-spin" aria-hidden />;
}

/** A quiet inline link out to GitHub — accent text, underline on hover, ↗. */
export function GitLink({ href, children }: { href: string; children: ReactNode }) {
  return (
    <button type="button" className="git-link iflex-center" onClick={() => void open(href)}>
      {children}
      <Icon name="external" size={10} />
    </button>
  );
}

/** The recurring "↗ View on GitHub" icon button (header, action bar, …). A
 *  convenience link, not an action — same tip/aria everywhere, only the
 *  wrapping class and glyph size vary by placement. */
export function ViewOnGitHub({
  href,
  className,
  size = 13,
}: {
  href: string;
  className: string;
  size?: number;
}) {
  return (
    <button
      type="button"
      className={`${className} tip`}
      data-tip="View on GitHub"
      aria-label="View on GitHub"
      onClick={() => void open(href)}
    >
      <Icon name="external" size={size} />
    </button>
  );
}
