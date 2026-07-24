import { useEffect, useMemo, useState } from "react";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Mono } from "@/components/SettingsScreen/CustomAgents/Mono";
import { Chip } from "@/components/ui/Chip";
import { Scrim } from "@/components/ui/Scrim";
import { PROVIDER_DETAIL } from "@/data/providerDetail";
import { isDockerSupported, PROVIDERS, providerLabel } from "@/data/providers";
import { useAppStore } from "@/store";

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
  const providerVersions = useAppStore((s) => s.providerVersions);
  const providerPaths = useAppStore((s) => s.providerPaths);
  const providersProbed = useAppStore((s) => s.providersProbed);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const customAgents = useAppStore((s) => s.customAgents);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);
  // New agents get the currently selected sandbox engine. Only providers with
  // container support (see `isDockerSupported`) run under Docker, so under the
  // docker engine every unsupported coding agent (and any custom agent whose
  // base isn't supported) is disabled — matching the backend spawn refusal in
  // supervisor/lifecycle.rs.
  const sandboxEngine = useAppStore((s) => s.sandboxEngine);
  const dockerOnly = sandboxEngine === "docker";

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

  function renderModelList(p: (typeof PROVIDERS)[number]) {
    const models = modelsByAgent[p.id] ?? [];
    // In model-only mode the list is always the session's own provider, so its
    // current model highlights even for a custom-agent session (customAgentId set).
    const isCurrent = modelOnly || (p.id === provider && !customAgentId);
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
                  <span className="model-ctx text-xs">{formatContext(m.contextWindow)}</span>
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
          <div
            className="model-dd-wrap"
            style={{ bottom: "calc(100% + 6px)", left: 0 }}
            onMouseLeave={() => setHovered(null)}
          >
            <div className="model-dd-main">
              <div className="model-sect flex-center text-xs">
                <span>Coding agents</span>
                <span className="model-sect-line" />
              </div>
              {enabled.map((p) => {
                // Fail open: only gate on the path once a probe has actually
                // succeeded (`providersProbed`). While probing, or if the probe
                // failed, treat as installed so a transient detection error
                // never disables an agent the user really has.
                const installed = !providersProbed || !!providerPaths[p.id];
                const missing = providersProbed && !installed;
                // Providers without container support can't run under Docker yet
                // (see dockerOnly + isDockerSupported).
                const dockerBlocked = dockerOnly && !isDockerSupported(p.id);
                const disabled = !installed || dockerBlocked;
                // Why disabled — dockerBlocked wins over missing (a non-Claude
                // agent under Docker is blocked regardless of install state).
                const disabledTip = dockerBlocked
                  ? `${p.label} isn't available in Docker sandboxes yet`
                  : "Not installed — see Settings › Providers";
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
                    data-tip={disabled ? disabledTip : undefined}
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
                    <span className="model-agent-ver text-xs">
                      {dockerBlocked
                        ? "Not in Docker yet"
                        : missing
                          ? "Not installed"
                          : providerVersions[p.id]}
                    </span>
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
                  // A custom agent inherits its base provider's docker support.
                  const dockerBlocked = dockerOnly && !isDockerSupported(a.base);
                  return (
                    <button
                      key={a.id}
                      type="button"
                      // Same reasoning as the provider rows above: aria-disabled
                      // (not the native attr) keeps the row hover-capable so the
                      // CSS .tip/data-tip refusal is reachable in the WebView.
                      aria-disabled={dockerBlocked}
                      data-tip={
                        dockerBlocked
                          ? `${providerLabel(a.base)} isn't available in Docker sandboxes yet`
                          : undefined
                      }
                      className={`model-custom-row flex-center ${dockerBlocked ? "is-disabled tip" : ""} ${active ? "active" : ""}`}
                      onMouseEnter={() => setHovered(null)}
                      onClick={() => !dockerBlocked && pickCustom(a.id, a.base, a.model)}
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
          <div className="model-dd-wrap" style={{ bottom: "calc(100% + 6px)", left: 0 }}>
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
