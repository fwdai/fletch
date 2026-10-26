import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Mono } from "@/components/SettingsScreen/CustomAgents/Mono";
import type { PROVIDERS } from "@/data/providers";
import { useAppStore } from "@/store";
import { activeEntry } from "@/store/capabilities";
import type { useAgentAvailability } from "../availability";
import { CUSTOM_FLY, useSelectableCustomAgents } from "./CustomAgentOptions";

type Provider = (typeof PROVIDERS)[number];
export type OpenSettings = ReturnType<typeof useAppStore.getState>["openSettingsScreen"];

/** The full picker's main list: the coding agents (hovering one opens its
 *  model flyout), then one row for the custom agents (hovering it opens their
 *  list the same way). */
export function AgentList({
  provider,
  customAgentId,
  enabled,
  installedCount,
  availability,
  hovered,
  setHovered,
  onPickModel,
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
  onOpenSettings: OpenSettings;
}) {
  const env = useAppStore(activeEntry);
  const selectableCustom = useSelectableCustomAgents();
  // Looked up in the offered list: one whose base is switched off is still the
  // chip's identity, but the row has nothing to open for it.
  const activeCustom = selectableCustom.find((a) => a.id === customAgentId);

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

      {/* One row for the custom agents, opening their list beside the card
       *  the way an agent row opens its models. Its note names the one in
       *  use, else how many there are to pick from. */}
      <button
        type="button"
        className={`model-agent-row model-custom-sum flex-center ${activeCustom ? "active" : ""} ${hovered === CUSTOM_FLY ? "hot" : ""}`}
        aria-haspopup="menu"
        aria-expanded={hovered === CUSTOM_FLY}
        title="Hover to choose a custom agent"
        onMouseEnter={() => setHovered(CUSTOM_FLY)}
        onClick={() => setHovered(CUSTOM_FLY)}
      >
        {activeCustom ? (
          <Mono name={activeCustom.name} hue={activeCustom.color} size={26} />
        ) : (
          <span className="model-custom-sum-icon">
            <Icon name="shapes" size={15} />
          </span>
        )}
        <span className="model-agent-name truncate text-base">Custom agents</span>
        <span className="model-agent-ver text-xs truncate">
          {activeCustom?.name ?? (selectableCustom.length > 0 ? selectableCustom.length : "none")}
        </span>
        <Icon name="chevR" size={12} />
      </button>
    </>
  );
}
