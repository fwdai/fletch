import type { ReactNode } from "react";

/** The panel that slides out from under a `FlyoutMenu`'s card, listing what
 *  the hovered row offers. `flyKey` names what it shows, so switching between
 *  rows fades the content in place. */
export function SideFlyout({
  flyKey,
  icon,
  title,
  tag,
  fill = false,
  children,
}: {
  flyKey: string;
  icon: ReactNode;
  title: string;
  /** What kind of thing the list picks, in a word, at the head's far end. */
  tag: string;
  /** The content lays itself out in the panel's height and scrolls what it
   *  chooses to (a list under a filter that stays put). Without it the whole
   *  content scrolls. */
  fill?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="fly-side">
      <div className="fly-side-card">
        <div className="fly-side-inner" key={flyKey}>
          <div className="fly-side-head flex-center">
            {icon}
            <span className="fly-side-name truncate text-base">{title}</span>
            <span className="fly-side-tag text-xs">{tag}</span>
          </div>
          <div className={`fly-side-list ${fill ? "is-fill" : ""}`}>{children}</div>
        </div>
      </div>
    </div>
  );
}
