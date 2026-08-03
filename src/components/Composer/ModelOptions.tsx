import { Icon } from "@/components/Icon";
import type { Provider } from "@/data/providers";
import { useAppStore } from "@/store";

function formatContext(tokens: number): string {
  if (!tokens) return "Context unknown";
  if (tokens >= 1_000_000) return `${Math.round(tokens / 1_000_000)}M ctx`;
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}k ctx`;
  return `${tokens} ctx`;
}

/** One provider's model catalog, as selectable rows: the provider default
 *  first, then every model the backend probed, with its context window.
 *
 *  Shared by the two surfaces that offer a model — the composer's picker, where
 *  it fills the flyout beside the agent list, and the Roadmap's new-chat screen,
 *  where it expands inline under the chosen agent. The rows are the same choice
 *  on both, and a second copy would drift. */
export function ModelOptions({
  provider,
  model,
  isCurrent,
  onPick,
}: {
  provider: Provider;
  /** The pinned model id, when the selection has one. */
  model?: string;
  /** These rows belong to the currently selected agent, so the active row is
   *  worth marking. A list shown for an agent that isn't selected — the
   *  composer's hover flyout — highlights nothing. */
  isCurrent: boolean;
  /** `undefined` model means "whatever the provider CLI defaults to". */
  onPick: (providerId: string, model: string | undefined) => void;
}) {
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const models = modelsByAgent[provider.id] ?? [];

  return (
    <div className="model-list">
      <button
        type="button"
        className={`model-option flex-center ${isCurrent && !model ? "active" : ""}`}
        onClick={(e) => {
          e.stopPropagation();
          onPick(provider.id, undefined);
        }}
      >
        <span className="model-option-main">
          <span className="model-option-name truncate def text-base">Default model</span>
          <span className="model-option-desc truncate text-xs">
            Use {provider.label}'s configured default
          </span>
        </span>
        {isCurrent && !model && <Icon name="check" size={13} />}
      </button>

      {models.length === 0 ? (
        <div className="model-empty text-sm">
          {provider.fixedModel
            ? `${provider.label} manages its own model — no selection needed.`
            : `Model catalog unavailable for ${provider.label}.`}
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
                onPick(provider.id, m.id);
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

/** The display name for a pinned model id, falling back to the "default model"
 *  wording every surface uses when nothing is pinned. */
export function useModelName(providerId: string, model: string | undefined): string {
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  if (!model) return "Default model";
  return modelsByAgent[providerId]?.find((m) => m.id === model)?.name ?? model;
}
