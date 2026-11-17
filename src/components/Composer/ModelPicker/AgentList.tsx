import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Mono } from "@/components/SettingsScreen/CustomAgents/Mono";
import { FlyoutGlyph, FlyoutRow, FlyoutSection } from "@/components/ui/FlyoutMenu";
import type { PROVIDERS } from "@/data/providers";
import type { useAppStore } from "@/store";
import { useHostEntry } from "@/store/capabilities";
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
  const env = useHostEntry();
  const selectableCustom = useSelectableCustomAgents();
  // Looked up in the offered list: one whose base is switched off is still the
  // chip's identity, but the row has nothing to open for it.
  const activeCustom = selectableCustom.find((a) => a.id === customAgentId);

  // Settings › Providers installs on this Mac only, so a paired host gets a note, not a button.
  const emptyState =
    env.kind === "remote" ? (
      <div className="fly-empty text-sm">
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
      <FlyoutSection>Coding agents</FlyoutSection>
      {enabled.length === 0 && emptyState}
      {enabled.map((p) => {
        // The shared gate the spawn path enforces.
        const { reason, note } = availability(p.id);
        const disabled = reason !== null;
        return (
          <FlyoutRow
            key={p.id}
            icon={<ProviderIcon slug={p.id} short={p.short} hue={p.hue} size={26} />}
            label={p.label}
            note={note}
            active={p.id === provider && !customAgentId}
            hot={hovered === p.id}
            disabled={disabled}
            tip={reason ?? undefined}
            chevron={!disabled}
            title={
              disabled ? undefined : "Click to use the default model · hover to choose a model"
            }
            onMouseEnter={() => !disabled && setHovered(p.id)}
            onClick={() => !disabled && onPickModel(p.id, undefined)}
          />
        );
      })}

      {/* One row for the custom agents, opening their list beside the card
       *  the way an agent row opens its models. Its note names the one in
       *  use, else how many there are to pick from. */}
      <FlyoutRow
        className="model-custom-sum"
        aria-haspopup="menu"
        aria-expanded={hovered === CUSTOM_FLY}
        title="Hover to choose a custom agent"
        icon={
          activeCustom ? (
            <Mono name={activeCustom.name} hue={activeCustom.color} size={26} />
          ) : (
            <FlyoutGlyph name="shapes" />
          )
        }
        label="Custom agents"
        note={
          activeCustom?.name ?? (selectableCustom.length > 0 ? selectableCustom.length : "none")
        }
        active={!!activeCustom}
        hot={hovered === CUSTOM_FLY}
        chevron
        onMouseEnter={() => setHovered(CUSTOM_FLY)}
        onClick={() => setHovered(CUSTOM_FLY)}
      />
    </>
  );
}
