import { Icon } from "@/components/Icon";
import { Mono } from "@/components/SettingsScreen/CustomAgents/Mono";
import { FlyoutOption } from "@/components/ui/FlyoutMenu";
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
    <div className="fly-list">
      {agents.map((a) => {
        const active = a.id === customAgentId;
        // A custom agent inherits its base provider's availability exactly.
        const { reason } = availability(a.base);
        const blocked = reason !== null;
        return (
          <FlyoutOption
            key={a.id}
            role="menuitemradio"
            aria-checked={active}
            // The refusal is the CSS `.tip`/`data-tip` one, shown on hover.
            className={blocked ? "tip" : undefined}
            data-tip={reason ?? undefined}
            title={a.name}
            lead={<Mono name={a.name} hue={a.color} size={22} />}
            name={a.name}
            desc={a.description || providerLabel(a.base)}
            active={active}
            disabled={blocked}
            onClick={(e) => {
              e.stopPropagation();
              if (!blocked) onPick(a.id, a.base, a.model);
            }}
          />
        );
      })}
      <FlyoutOption
        className="fly-manage"
        name="Manage custom agents…"
        trail={<Icon name="settings" size={13} />}
        onClick={() => onOpenSettings("agents")}
      />
    </div>
  );
}
