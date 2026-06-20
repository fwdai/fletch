import { useEffect, useMemo, useState } from "react";
import { PROVIDERS } from "../../data/providers";
import { hasAdapter } from "../../adapters";
import { useAppStore } from "../../store";
import { Icon } from "../Icon";
import { ProviderIcon } from "../ProviderIcon";
import { Chip } from "../ui/Chip";
import { Scrim } from "../ui/Scrim";

interface Props {
  provider: string;
  model?: string;
  onChange: (provider: string, model?: string) => void;
  locked?: boolean;
}

function formatContext(tokens: number): string {
  if (!tokens) return "Context unknown";
  if (tokens >= 1_000_000) return `${Math.round(tokens / 1_000_000)}M ctx`;
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}k ctx`;
  return `${tokens} ctx`;
}

function modelFamily(name: string): string {
  const first = name.split(/\s+/)[0];
  return first || "Model";
}

/** Agent + model picker. The left rail previews each runner's models; selecting
 *  a row in the right pane commits the runner/model choice. Leaving model unset
 *  preserves the provider CLI's default. */
export function ModelPicker({ provider, model, onChange, locked = false }: Props) {
  const [open, setOpen] = useState(false);
  const [focusProvider, setFocusProvider] = useState(provider);
  const providerFlags = useAppStore((s) => s.providerFlags);
  const providerVersions = useAppStore((s) => s.providerVersions);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);

  const selected = PROVIDERS.find((p) => p.id === provider) ?? PROVIDERS[0];
  const enabled = PROVIDERS.filter((p) => providerFlags[p.id] !== false);
  const focused = PROVIDERS.find((p) => p.id === focusProvider) ?? selected;
  const focusedModels = modelsByAgent[focused.id] ?? [];
  const currentModel = useMemo(() => {
    const list = modelsByAgent[provider] ?? [];
    return list.find((m) => m.id === model);
  }, [model, modelsByAgent, provider]);

  useEffect(() => {
    if (open) setFocusProvider(provider);
  }, [open, provider]);

  function pickModel(id: string | undefined) {
    onChange(focused.id, id);
    setOpen(false);
  }

  return (
    <div className="model-picker">
      <Chip
        bordered
        disabled={locked}
        onClick={() => {
          if (!locked) setOpen((v) => !v);
        }}
        tip={locked ? selected.label : "Agent and model"}
        className="model-chip"
      >
        <ProviderIcon
          slug={selected.id}
          short={selected.short}
          hue={selected.hue}
          size={15}
        />
        <span className="model-chip-agent">{selected.label}</span>
        <span className="model-chip-model">
          {currentModel?.name ?? "Default model"}
        </span>
        {!locked && <Icon name="chevD" size={9} />}
      </Chip>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <div className="dd model-dd" style={{ bottom: "calc(100% + 6px)", left: 0 }}>
            <div className="model-dd-head">
              <span>Agent</span>
              <span>Model</span>
            </div>
            <div className="model-dd-grid">
              <div className="model-agent-list">
                {enabled.map((p) => {
                  const wired = hasAdapter(p.id);
                  return (
                    <button
                      key={p.id}
                      type="button"
                      className={`model-agent ${p.id === focusProvider ? "active" : ""}`}
                      disabled={!wired}
                      onClick={() => wired && setFocusProvider(p.id)}
                    >
                      <ProviderIcon slug={p.id} short={p.short} hue={p.hue} size={18} />
                      <span className="model-agent-text">
                        <span>{p.label}</span>
                        <span>{providerVersions[p.id] ?? p.version}</span>
                      </span>
                      {p.id === provider && <Icon name="check" size={12} />}
                    </button>
                  );
                })}
              </div>

              <div className="model-list">
                <button
                  type="button"
                  className={`model-option ${focused.id === provider && !model ? "active" : ""}`}
                  onClick={() => pickModel(undefined)}
                >
                  <span className="model-mark">
                    {focused.id === provider && !model && <Icon name="check" size={12} />}
                  </span>
                  <span className="model-option-main">
                    <span>Default model</span>
                    <span>Use {focused.label}'s configured default</span>
                  </span>
                </button>

                {focusedModels.length === 0 ? (
                  <div className="model-empty">
                    Model catalog unavailable for {focused.label}.
                  </div>
                ) : (
                  focusedModels.map((m) => {
                    const active = focused.id === provider && m.id === model;
                    return (
                      <button
                        key={m.id}
                        type="button"
                        className={`model-option ${active ? "active" : ""}`}
                        onClick={() => pickModel(m.id)}
                      >
                        <span className="model-mark">
                          {active && <Icon name="check" size={12} />}
                        </span>
                        <span className="model-option-main">
                          <span>{m.name}</span>
                          <span>{m.id}</span>
                        </span>
                        <span className="model-option-meta">
                          <span>{modelFamily(m.name)}</span>
                          <span>{formatContext(m.contextWindow)}</span>
                          {m.reasoning && <span>reasoning</span>}
                        </span>
                      </button>
                    );
                  })
                )}
              </div>
            </div>
            {currentModel && (
              <div className="model-dd-foot">
                {selected.label} · {currentModel.name} · {formatContext(currentModel.contextWindow)}
              </div>
            )}
          </div>
        </>
      )}
    </div>
  );
}
