import { useEffect, useMemo, useState } from "react";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Mono } from "@/components/SettingsScreen/CustomAgents/Mono";
import { Chip } from "@/components/ui/Chip";
import { Scrim } from "@/components/ui/Scrim";
import { PROVIDER_DETAIL } from "@/data/providerDetail";
import { PROVIDERS, providerLabel } from "@/data/providers";
import { useAppStore } from "@/store";
import { useAgentAvailability } from "./availability";
import { ModelOptions } from "./ModelOptions";

interface Props {
  provider: string;
  model?: string;
  /** Selected custom agent id, if the user picked one rather than a built-in
   *  provider. Drives the chip's identity and the dropdown's active row. */
  customAgentId?: string;
  onChange: (provider: string, model?: string, customAgentId?: string) => void;
  locked?: boolean;
  /** Existing sessions: restrict the picker to changing the MODEL within the
   *  session's current provider. Provider and custom-agent identity are fixed
   *  at spawn, so the dropdown drops the agent/custom-agent sections and shows
   *  only this provider's models. */
  modelOnly?: boolean;
}

/** Agent + model picker for the composer. A flat list groups coding agents and
 *  custom agents; hovering a coding agent opens a flyout on the right for model
 *  selection. Clicking an agent row commits its default model; leaving model
 *  unset preserves the provider CLI's default. Selections stay sticky via
 *  `onChange`.
 *
 *  The menu always opens upward — the composer sits on the bottom edge of the
 *  window. A surface near the top of a panel wants a screen, not a menu that has
 *  to grow the other way (see the Roadmap's `NewChatScreen`). */
export function ModelPicker({
  provider,
  model,
  customAgentId,
  onChange,
  locked = false,
  modelOnly = false,
}: Props) {
  const [open, setOpen] = useState(false);
  // Coding agent whose model flyout is currently expanded (null = none).
  const [hovered, setHovered] = useState<string | null>(null);
  const providerFlags = useAppStore((s) => s.providerFlags);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const customAgents = useAppStore((s) => s.customAgents);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);
  // Whether each agent can actually be spawned right now — installed, and
  // container-ready when the Docker engine is on. Matches the backend refusal
  // in supervisor/lifecycle.rs.
  const availability = useAgentAvailability();

  const selected = PROVIDERS.find((p) => p.id === provider) ?? PROVIDERS[0];
  const enabled = PROVIDERS.filter((p) => providerFlags[p.id] !== false);
  const currentModel = useMemo(() => {
    const list = modelsByAgent[provider] ?? [];
    return list.find((m) => m.id === model);
  }, [model, modelsByAgent, provider]);

  // The active custom agent (chip identity + dropdown highlight). Only custom
  // agents whose base provider is enabled are offered.
  //
  // `activeCustom` is looked up against the full library on purpose: it drives
  // the chip, and a selection made elsewhere must still render with its own
  // name rather than silently reading as a bare provider.
  const activeCustom = customAgents.find((a) => a.id === customAgentId);
  const selectableCustom = customAgents.filter((a) => providerFlags[a.base] !== false);
  // The coding agent whose model panel is currently shown (null = none).
  const hoveredAgent = hovered ? (PROVIDERS.find((p) => p.id === hovered) ?? null) : null;

  // Reset the flyout each time the dropdown opens.
  useEffect(() => {
    if (open) setHovered(null);
  }, [open]);

  // Model-only (existing session): changing the model on a provider that bakes
  // it into the process (claude, `restartToApply`) restarts the agent; surface
  // that in the chip tooltip, mirroring the effort chip.
  const restartOnChange =
    modelOnly && !!PROVIDER_DETAIL[provider as keyof typeof PROVIDER_DETAIL]?.restartToApply;
  const chipTip = locked
    ? (activeCustom?.name ?? selected.label)
    : modelOnly
      ? restartOnChange
        ? "Model — changing restarts the agent (rebuilds cache)"
        : "Model"
      : "Agent and model";

  function pickModel(providerId: string, id: string | undefined) {
    // A model-only pick (existing session) keeps the session's custom-agent
    // identity; a full-picker pick clears it.
    onChange(providerId, id, modelOnly ? customAgentId : undefined);
    setOpen(false);
  }

  function pickCustom(agentId: string, base: string, agentModel: string | null) {
    onChange(base, agentModel ?? undefined, agentId);
    setOpen(false);
  }

  /** The list for one provider. In model-only mode it is always the session's
   *  own provider, so its current model highlights even for a custom-agent
   *  session (customAgentId set). */
  function renderModelList(p: (typeof PROVIDERS)[number]) {
    return (
      <ModelOptions
        provider={p}
        model={model}
        isCurrent={modelOnly || (p.id === provider && !customAgentId)}
        onPick={pickModel}
      />
    );
  }

  const customSection = (
    <>
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
              onClick={() => !blocked && pickCustom(a.id, a.base, a.model)}
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
          onClick={() => {
            setOpen(false);
            openSettingsScreen("agents", "new-custom-agent");
          }}
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

  return (
    <div className="model-picker">
      <Chip
        bordered
        disabled={locked}
        onClick={() => {
          if (!locked) setOpen((v) => !v);
        }}
        tip={chipTip}
        className="model-chip"
      >
        {activeCustom ? (
          <>
            <Mono name={activeCustom.name} hue={activeCustom.color} size={15} />
            <span className="model-chip-agent">{activeCustom.name}</span>
            <span className="model-chip-model truncate">{providerLabel(activeCustom.base)}</span>
          </>
        ) : (
          <>
            <ProviderIcon slug={selected.id} short={selected.short} hue={selected.hue} size={15} />
            <span className="model-chip-agent">{selected.label}</span>
            <span className="model-chip-model truncate">
              {currentModel?.name ?? "Default model"}
            </span>
          </>
        )}
        {!locked && <Icon name="chevD" size={9} />}
      </Chip>

      {open && !modelOnly && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          {/* Transparent wrapper: the main card sits above (z-index 2) the
           *  model side panel (z-index 1) so the panel slides out from
           *  underneath the dropdown rather than floating beside it. */}
          <div className="model-dd-wrap" onMouseLeave={() => setHovered(null)}>
            <div className="model-dd-main">
              <div className="model-sect flex-center text-xs">
                <span>Coding agents</span>
                <span className="model-sect-line" />
              </div>
              {enabled.map((p) => {
                // Installed, and container-ready if Docker is on — the shared
                // gate the spawn path enforces.
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
                      disabled
                        ? undefined
                        : "Click to use the default model · hover to choose a model"
                    }
                    onMouseEnter={() => !disabled && setHovered(p.id)}
                    onClick={() => !disabled && pickModel(p.id, undefined)}
                  >
                    <ProviderIcon slug={p.id} short={p.short} hue={p.hue} size={26} />
                    <span className="model-agent-name truncate text-base">{p.label}</span>
                    <span className="model-agent-ver text-xs">{note}</span>
                    {!disabled && <Icon name="chevR" size={12} />}
                  </button>
                );
              })}

              {customSection}
            </div>

            {hoveredAgent && (
              <div className="model-side-fly">
                <div className="model-side-fly-card">
                  <div className="model-side-fly-inner" key={hoveredAgent.id}>
                    <div className="model-side-fly-head flex-center">
                      <ProviderIcon
                        slug={hoveredAgent.id}
                        short={hoveredAgent.short}
                        hue={hoveredAgent.hue}
                        size={20}
                      />
                      <span className="model-side-fly-name truncate text-base">
                        {hoveredAgent.label}
                      </span>
                      <span className="model-side-fly-tag text-xs">model</span>
                    </div>
                    <div className="model-side-fly-list">{renderModelList(hoveredAgent)}</div>
                  </div>
                </div>
              </div>
            )}
          </div>
        </>
      )}

      {open && modelOnly && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <div className="model-dd-wrap">
            <div className="model-dd-main">
              <div className="model-sect flex-center text-xs">
                <span>Model</span>
                <span className="model-sect-line" />
              </div>
              {renderModelList(selected)}
            </div>
          </div>
        </>
      )}
    </div>
  );
}
