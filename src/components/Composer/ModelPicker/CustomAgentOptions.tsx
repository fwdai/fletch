import { Icon } from "@/components/Icon";
import { Mono } from "@/components/SettingsScreen/CustomAgents/Mono";
import { providerLabel } from "@/data/providers";
import { useAppStore } from "@/store";
import type { useAgentAvailability } from "../availability";
import type { OpenSettings } from "./AgentList";

/** `hovered` while the custom-agents flyout is the one open; no provider id. */
export const CUSTOM_FLY = "__custom";

/** The custom agents the picker offers: those whose base provider is on. */
export function useSelectableCustomAgents() {
  const providerFlags = useAppStore((s) => s.providerFlags);
  const customAgents = useAppStore((s) => s.customAgents);
  return customAgents.filter((a) => providerFlags[a.base] !== false);
}

/** The custom-agents flyout: every custom agent configured in Settings whose
 *  base provider is on, the current one checked, then a row to the settings
 *  pane. With none set up, the row that creates the first one. */
export function CustomAgentOptions({
  customAgentId,
  availability,
  onPick,
  onOpenSettings,
}: {
  customAgentId?: string;
  availability: ReturnType<typeof useAgentAvailability>;
  onPick: (agentId: string, base: string, model: string | null) => void;
  onOpenSettings: OpenSettings;
}) {
  const agents = useSelectableCustomAgents();

  if (agents.length === 0) {
    return (
      <button
        type="button"
        className="model-custom-cta flex-center"
        onClick={() => onOpenSettings("agents", "new-custom-agent")}
      >
        <span className="model-custom-cta-icon">
          <Icon name="plus" size={14} />
        </span>
        <span className="model-custom-text">
          <span>Set up a custom agent</span>
          <span>Pair an agent with a model and a standing brief</span>
        </span>
      </button>
    );
  }

  return (
    <div className="model-list">
      {agents.map((a) => {
        const active = a.id === customAgentId;
        // A custom agent inherits its base provider's availability exactly.
        const { reason } = availability(a.base);
        const blocked = reason !== null;
        return (
          <button
            key={a.id}
            type="button"
            role="menuitemradio"
            aria-checked={active}
            // aria-disabled, not the native attr: a disabled button swallows
            // hover in the WebView, and the .tip/data-tip refusal must stay
            // reachable.
            aria-disabled={blocked}
            data-tip={reason ?? undefined}
            title={a.name}
            className={`model-option flex-center ${blocked ? "is-disabled tip" : ""} ${active ? "active" : ""}`}
            onClick={(e) => {
              e.stopPropagation();
              if (!blocked) onPick(a.id, a.base, a.model);
            }}
          >
            <Mono name={a.name} hue={a.color} size={22} />
            <span className="model-option-main">
              <span className="model-option-name truncate text-base">{a.name}</span>
              <span className="model-option-desc truncate text-xs">
                {a.description || providerLabel(a.base)}
              </span>
            </span>
            {active && <Icon name="check" size={13} />}
          </button>
        );
      })}
      <button
        type="button"
        className="model-option model-fly-manage flex-center"
        onClick={() => onOpenSettings("agents")}
      >
        <span className="model-option-main">
          <span className="model-option-name text-base">Manage custom agents…</span>
        </span>
        <Icon name="settings" size={13} />
      </button>
    </div>
  );
}
