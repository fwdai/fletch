import type { ReactNode } from "react";
import { ProviderIcon } from "@/components/ProviderIcon";
import type { PROVIDERS } from "@/data/providers";

/** The panel that slides out from under the main card with the hovered coding
 *  agent's models. */
export function ModelFlyout({
  agent,
  children,
}: {
  agent: (typeof PROVIDERS)[number];
  children: ReactNode;
}) {
  return (
    <div className="model-side-fly">
      <div className="model-side-fly-card">
        <div className="model-side-fly-inner" key={agent.id}>
          <div className="model-side-fly-head flex-center">
            <ProviderIcon slug={agent.id} short={agent.short} hue={agent.hue} size={20} />
            <span className="model-side-fly-name truncate text-base">{agent.label}</span>
            <span className="model-side-fly-tag text-xs">model</span>
          </div>
          <div className="model-side-fly-list">{children}</div>
        </div>
      </div>
    </div>
  );
}
