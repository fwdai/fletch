import { useEffect, useMemo, useState } from "react";
import { hasAdapter } from "../../adapters";
import { PROVIDERS, providerLabel } from "../../data/providers";
import { useAppStore } from "../../store";
import { Icon } from "../Icon";
import { ProviderIcon } from "../ProviderIcon";
import { Mono } from "../SettingsScreen/CustomAgents/Mono";
import { Chip } from "../ui/Chip";
import { Scrim } from "../ui/Scrim";

interface Props {
  provider: string;
  model?: string;
  /** Selected custom agent id, if the user picked one rather than a built-in
   *  provider. Drives the chip's identity and the dropdown's active row. */
  customAgentId?: string;
  onChange: (provider: string, model?: string, customAgentId?: string) => void;
  locked?: boolean;
}

function formatContext(tokens: number): string {
  if (!tokens) return "Context unknown";
  if (tokens >= 1_000_000) return `${Math.round(tokens / 1_000_000)}M ctx`;
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}k ctx`;
  return `${tokens} ctx`;
}

/** Agent + model picker. A flat list groups coding agents and custom agents;
 *  hovering a coding agent opens a flyout on the right for model selection.
 *  Clicking an agent row commits its default model; leaving model unset
 *  preserves the provider CLI's default. Selections stay sticky via `onChange`. */
export function ModelPicker({ provider, model, customAgentId, onChange, locked = false }: Props) {
  const [open, setOpen] = useState(false);
  // Coding agent whose model flyout is currently expanded (null = none).
  const [hovered, setHovered] = useState<string | null>(null);
  const providerFlags = useAppStore((s) => s.providerFlags);
  const providerVersions = useAppStore((s) => s.providerVersions);
  const providerPaths = useAppStore((s) => s.providerPaths);
  const providersProbed = useAppStore((s) => s.providersProbed);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const customAgents = useAppStore((s) => s.customAgents);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);

  const selected = PROVIDERS.find((p) => p.id === provider) ?? PROVIDERS[0];
  const enabled = PROVIDERS.filter((p) => providerFlags[p.id] !== false);
  const currentModel = useMemo(() => {
    const list = modelsByAgent[provider] ?? [];
    return list.find((m) => m.id === model);
  }, [model, modelsByAgent, provider]);

  // The active custom agent (chip identity + dropdown highlight). Only custom
  // agents whose base provider is enabled are offered.
  const activeCustom = customAgents.find((a) => a.id === customAgentId);
  const selectableCustom = customAgents.filter((a) => providerFlags[a.base] !== false);
  // The coding agent whose model panel is currently shown (null = none).
  const hoveredAgent = hovered ? (PROVIDERS.find((p) => p.id === hovered) ?? null) : null;

  // Reset the flyout each time the dropdown opens.
  useEffect(() => {
    if (open) setHovered(null);
  }, [open]);

  function pickModel(providerId: string, id: string | undefined) {
    // Selecting a built-in model clears any custom-agent selection.
    onChange(providerId, id, undefined);
    setOpen(false);
  }

  function pickCustom(agentId: string, base: string, agentModel: string | null) {
    onChange(base, agentModel ?? undefined, agentId);
    setOpen(false);
  }

  function renderModelList(p: (typeof PROVIDERS)[number]) {
    const models = modelsByAgent[p.id] ?? [];
    const isCurrent = p.id === provider && !customAgentId;
    return (
      <div className="model-list">
        <button
          type="button"
          className={`model-option flex-center ${isCurrent && !model ? "active" : ""}`}
          onClick={(e) => {
            e.stopPropagation();
            pickModel(p.id, undefined);
          }}
        >
          <span className="model-option-main">
            <span className="model-option-name truncate def text-base">Default model</span>
            <span className="model-option-desc truncate text-xs">
              Use {p.label}'s configured default
            </span>
          </span>
          {isCurrent && !model && <Icon name="check" size={13} />}
        </button>

        {models.length === 0 ? (
          <div className="model-empty text-sm">
            {p.fixedModel
              ? `${p.label} manages its own model — no selection needed.`
              : `Model catalog unavailable for ${p.label}.`}
          </div>
        ) : (
          models.map((m) => {
            const active = isCurrent && m.id === model;
            return (
              <button
                key={m.id}
                type="button"
                className={`model-option flex-center ${active ? "active" : ""}`}
                onClick={(e) => {
                  e.stopPropagation();
                  pickModel(p.id, m.id);
                }}
              >
                <span className="model-option-main">
                  <span className="model-option-name truncate text-base">{m.name}</span>
                </span>
                {m.contextWindow > 0 && (
                  <span className="model-ctx text-2xs">{formatContext(m.contextWindow)}</span>
                )}
                {active && <Icon name="check" size={13} />}
              </button>
            );
          })
        )}
      </div>
    );
  }

  return (
    <div className="model-picker">
      <Chip
        bordered
        disabled={locked}
        onClick={() => {
          if (!locked) setOpen((v) => !v);
        }}
        tip={locked ? (activeCustom?.name ?? selected.label) : "Agent and model"}
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

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          {/* Transparent wrapper: the main card sits above (z-index 2) the
           *  model side panel (z-index 1) so the panel slides out from
           *  underneath the dropdown rather than floating beside it. */}
          <div
            className="model-dd-wrap"
            style={{ bottom: "calc(100% + 6px)", left: 0 }}
            onMouseLeave={() => setHovered(null)}
          >
            <div className="model-dd-main">
              <div className="model-sect flex-center text-2xs">
                <span>Coding agents</span>
                <span className="model-sect-line" />
              </div>
              {enabled.map((p) => {
                const wired = hasAdapter(p.id);
                // Fail open: only gate on the path once a probe has actually
                // succeeded (`providersProbed`). While probing, or if the probe
                // failed, treat as installed so a transient detection error
                // never disables an agent the user really has.
                const installed = !providersProbed || !!providerPaths[p.id];
                const usable = wired && installed;
                const missing = wired && providersProbed && !installed;
                const isSelected = p.id === provider && !customAgentId;
                const isOpen = hovered === p.id;
                return (
                  <button
                    key={p.id}
                    type="button"
                    disabled={!usable}
                    className={`model-agent-row flex-center ${isSelected ? "active" : ""} ${isOpen ? "hot" : ""}`}
                    title={
                      missing
                        ? "Not installed — see Settings › Providers"
                        : "Click to use the default model · hover to choose a model"
                    }
                    onMouseEnter={() => usable && setHovered(p.id)}
                    onClick={() => usable && pickModel(p.id, undefined)}
                  >
                    <ProviderIcon slug={p.id} short={p.short} hue={p.hue} size={26} />
                    <span className="model-agent-name truncate text-base">{p.label}</span>
                    <span className="model-agent-ver text-2xs">
                      {missing ? "Not installed" : (providerVersions[p.id] ?? p.version)}
                    </span>
                    {usable && <Icon name="chevR" size={12} />}
                  </button>
                );
              })}

              <div className="model-sect flex-center text-2xs">
                <span>Custom agents</span>
                <span className="model-sect-line" />
              </div>
              {selectableCustom.length > 0 ? (
                selectableCustom.map((a) => {
                  const active = a.id === customAgentId;
                  return (
                    <button
                      key={a.id}
                      type="button"
                      className={`model-custom-row flex-center ${active ? "active" : ""}`}
                      onMouseEnter={() => setHovered(null)}
                      onClick={() => pickCustom(a.id, a.base, a.model)}
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
                      <span className="model-side-fly-tag text-2xs">model</span>
                    </div>
                    <div className="model-side-fly-list">{renderModelList(hoveredAgent)}</div>
                  </div>
                </div>
              </div>
            )}
          </div>
        </>
      )}
    </div>
  );
}
