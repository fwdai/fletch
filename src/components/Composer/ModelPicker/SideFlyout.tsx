import type { ReactNode } from "react";

/** The panel that slides out from under the main card: the hovered coding
 *  agent's models, or the agent's accounts. `flyKey` names what it shows, so
 *  switching between them fades the content in place. */
export function SideFlyout({
  flyKey,
  icon,
  title,
  tag,
  children,
}: {
  flyKey: string;
  icon: ReactNode;
  title: string;
  tag: string;
  children: ReactNode;
}) {
  return (
    <div className="model-side-fly">
      <div className="model-side-fly-card">
        <div className="model-side-fly-inner" key={flyKey}>
          <div className="model-side-fly-head flex-center">
            {icon}
            <span className="model-side-fly-name truncate text-base">{title}</span>
            <span className="model-side-fly-tag text-xs">{tag}</span>
          </div>
          <div className="model-side-fly-list">{children}</div>
        </div>
      </div>
    </div>
  );
}
