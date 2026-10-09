import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Mono } from "@/components/SettingsScreen/CustomAgents/Mono";
import { type PROVIDERS, providerLabel } from "@/data/providers";
import { useAppStore } from "@/store";
import { activeEntry } from "@/store/capabilities";
import type { useAgentAvailability } from "../availability";

type Provider = (typeof PROVIDERS)[number];
export type OpenSettings = ReturnType<typeof useAppStore.getState>["openSettingsScreen"];

/** The full picker's main list: the coding agents (hovering one opens its
 *  model flyout), then the custom agents. */
export function AgentList({
  provider,
  customAgentId,
  enabled,
  installedCount,
  availability,
  hovered,
  setHovered,
  onPickModel,
  onPickCustom,
  onOpenSettings,
}: {
  provider: string;
  customAgentId?: string;
  /** Installed agents the user hasn't switched off. */
  enabled: Provider[];
  installedCount: number;
  availability: ReturnType<typeof useAgentAvailability>;
  hovered: string | null;
  setHovered: (id: string | null) => void;
  onPickModel: (providerId: string, model: string | undefined) => void;
  onPickCustom: (agentId: string, base: string, model: string | null) => void;
  onOpenSettings: OpenSettings;
}) {
  const providerFlags = useAppStore((s) => s.providerFlags);
  const customAgents = useAppStore((s) => s.customAgents);
  const env = useAppStore(activeEntry);
  // Only custom agents whose base provider is enabled are offered.
  const selectableCustom = customAgents.filter((a) => providerFlags[a.base] !== false);

  // Settings › Providers installs on this Mac only, so a paired host gets a note, not a button.
  const emptyState =
    env.kind === "remote" ? (
      <div className="model-empty text-sm">
        No coding agents installed on {env.name}. Install one on the host and it appears here.
      </div>
    ) : (
      <button
        type="button"
        className="model-custom-cta flex-center"
        onMouseEnter={() => setHovered(null)}
        onClick={() => onOpenSettings("providers")}
      >
        <span className="model-custom-cta-icon">
          <Icon name={installedCount > 0 ? "blocks" : "arrowDown"} size={14} />
        </span>
        <span className="model-custom-text">
          {installedCount > 0 ? (
            <>
              <span>All coding agents are switched off</span>
              <span>Turn one on in Settings › Providers</span>
            </>
          ) : (
            <>
              <span>No coding agents installed</span>
              <span>Install one in Settings › Providers</span>
            </>
          )}
        </span>
      </button>
    );

  return (
    <>
      <div className="model-sect flex-center text-xs">
        <span>Coding agents</span>
        <span className="model-sect-line" />
      </div>
      {enabled.length === 0 && emptyState}
      {enabled.map((p) => {
        // Container-ready if Docker is on — the shared gate the spawn
        // path enforces.
        const { reason, note } = availability(p.id);
        const disabled = reason !== null;
        const isSelected = p.id === provider && !customAgentId;
        const isOpen = hovered === p.id;
        return (
          <button
            key={p.id}
            type="button"
            // aria-disabled, not the native `disabled` attr: a disabled
            // <button> swallows hover/pointer events in the WebView, so
            // its tooltip never shows and the user is left with only the
            // "Not in Docker yet" chip and no reason. aria-disabled keeps
            // the row hover-capable; the guarded handlers below keep it
            // inert. The tooltip is the CSS `.tip`/`data-tip` one
            // (shows on :hover), used only for the disabled explanation.
            aria-disabled={disabled}
            className={`model-agent-row flex-center ${disabled ? "is-disabled tip" : ""} ${isSelected ? "active" : ""} ${isOpen ? "hot" : ""}`}
            data-tip={reason ?? undefined}
            title={
              disabled ? undefined : "Click to use the default model · hover to choose a model"
            }
            onMouseEnter={() => !disabled && setHovered(p.id)}
            onClick={() => !disabled && onPickModel(p.id, undefined)}
          >
            <ProviderIcon slug={p.id} short={p.short} hue={p.hue} size={26} />
            <span className="model-agent-name truncate text-base">{p.label}</span>
            <span className="model-agent-ver text-xs">{note}</span>
            {!disabled && <Icon name="chevR" size={12} />}
          </button>
        );
      })}

      <div className="model-sect flex-center text-xs">
        <span>Custom agents</span>
        <span className="model-sect-line" />
      </div>
      {selectableCustom.length > 0 ? (
        selectableCustom.map((a) => {
          const active = a.id === customAgentId;
          // A custom agent inherits its base provider's availability exactly.
          const { reason } = availability(a.base);
          const blocked = reason !== null;
          return (
            <button
              key={a.id}
              type="button"
              // Same reasoning as the provider rows: aria-disabled (not the
              // native attr) keeps the row hover-capable so the CSS
              // .tip/data-tip refusal is reachable in the WebView.
              aria-disabled={blocked}
              data-tip={reason ?? undefined}
              className={`model-custom-row flex-center ${blocked ? "is-disabled tip" : ""} ${active ? "active" : ""}`}
              onMouseEnter={() => setHovered(null)}
              onClick={() => !blocked && onPickCustom(a.id, a.base, a.model)}
            >
              <Mono name={a.name} hue={a.color} size={26} />
              <span className="model-custom-text">
                <span>{a.name}</span>
                <span>{a.description || providerLabel(a.base)}</span>
              </span>
              {active && <Icon name="check" size={12} />}
            </button>
          );
        })
      ) : (
        <button
          type="button"
          className="model-custom-cta flex-center"
          onMouseEnter={() => setHovered(null)}
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
      )}
    </>
  );
}
