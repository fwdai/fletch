import { ModelOptions, useModelName } from "@/components/Composer/ModelOptions";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Mono } from "@/components/SettingsScreen/CustomAgents/Mono";
import type { AgentOption } from "./options";

/** One agent, as a selectable card.
 *
 *  The model lives *inside* the card rather than in a flyout: only the selected
 *  card offers it, and expanding it grows the card downward. That keeps the
 *  whole choice on one plane — the surface this screen replaced stacked a
 *  popover, a menu and a hover flyout to say the same thing, and the innermost
 *  layer opened off the top of the window. */
export function AgentCard({
  option,
  selected,
  model,
  modelsOpen,
  onSelect,
  onToggleModels,
  onPickModel,
}: {
  option: AgentOption;
  selected: boolean;
  /** The pinned model id for this option, when the user has chosen one. */
  model: string | undefined;
  modelsOpen: boolean;
  onSelect: () => void;
  onToggleModels: () => void;
  onPickModel: (model: string | undefined) => void;
}) {
  const { provider, custom, disabled } = option;
  const modelName = useModelName(option.pick.provider, model);
  // A custom agent's model is part of its definition, and a fixed-model
  // provider has nothing to choose — neither gets the control.
  const offersModel = !!provider && !provider.fixedModel && !disabled;
  const sub = disabled ?? (selected && provider && !provider.fixedModel ? modelName : option.desc);

  return (
    <div
      className={`rm-nc-card ${selected ? "active" : ""} ${disabled ? "is-disabled" : ""} ${modelsOpen ? "open" : ""}`}
    >
      <div className="rm-nc-row flex-center">
        <button
          type="button"
          role="radio"
          aria-checked={selected}
          // aria-disabled, not the native attr: a disabled <button> swallows
          // hover in the WebView, so its refusal tooltip would never show.
          // Same trade the composer's picker makes.
          aria-disabled={!!disabled}
          data-tip={disabled ?? undefined}
          // Roving tabstop: the group is one stop, arrows move within it.
          tabIndex={selected ? 0 : -1}
          className={`rm-nc-pick flex-center ${disabled ? "tip" : ""}`}
          onClick={() => !disabled && onSelect()}
        >
          <span className="rm-nc-mark">
            {custom ? (
              <Mono name={custom.name} hue={custom.color} size={28} />
            ) : provider ? (
              <ProviderIcon
                slug={provider.id}
                short={provider.short}
                hue={provider.hue}
                size={28}
              />
            ) : null}
          </span>
          <span className="rm-nc-text">
            <span className="rm-nc-name truncate text-base">{option.name}</span>
            {/* Once selected, a provider card says which model it will run — the
                pinned one, or its default. A provider the probe found no version
                for has nothing to say until then, and draws no second line. */}
            {sub && <span className="rm-nc-desc truncate text-xs">{sub}</span>}
          </span>
          {selected && !disabled && <Icon name="check" size={13} />}
        </button>

        {selected && offersModel && (
          <button
            type="button"
            className="rm-nc-model flex-center text-xs"
            aria-expanded={modelsOpen}
            onClick={onToggleModels}
          >
            Model
            <Icon name={modelsOpen ? "chevU" : "chevD"} size={9} />
          </button>
        )}
      </div>

      {selected && modelsOpen && provider && (
        <div className="rm-nc-models">
          <ModelOptions
            provider={provider}
            model={model}
            isCurrent
            onPick={(_providerId, next) => onPickModel(next)}
          />
        </div>
      )}
    </div>
  );
}
