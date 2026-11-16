import { FlyoutOption } from "@/components/ui/FlyoutMenu";
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
    <div className="fly-list">
      <FlyoutOption
        strong
        name="Default model"
        desc={`Use ${provider.label}'s configured default`}
        active={isCurrent && !model}
        onClick={(e) => {
          e.stopPropagation();
          onPick(provider.id, undefined);
        }}
      />

      {models.length === 0 ? (
        <div className="fly-empty text-sm">
          {provider.fixedModel
            ? `${provider.label} manages its own model — no selection needed.`
            : `Model catalog unavailable for ${provider.label}.`}
        </div>
      ) : (
        models.map((m) => (
          <FlyoutOption
            key={m.id}
            name={m.name}
            trail={
              m.contextWindow > 0 && (
                <span className="model-ctx text-xs">{formatContext(m.contextWindow)}</span>
              )
            }
            active={isCurrent && m.id === model}
            onClick={(e) => {
              e.stopPropagation();
              onPick(provider.id, m.id);
            }}
          />
        ))
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
