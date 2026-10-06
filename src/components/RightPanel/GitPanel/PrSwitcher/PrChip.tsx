import type { KeyboardEvent, Ref } from "react";
import type { PrSetEntry } from "@/api";
import { Icon } from "@/components/Icon";
import { Badge } from "@/components/ui/Badge";
import { prTint } from "@/util/prState";

/** `#N · title · branch · status`, skipping what the host doesn't know — the
 *  chip's native tooltip, and the menu row's accessible name. */
export function prSummary({ state, checks }: PrSetEntry): string {
  return [`#${state.number}`, state.title, state.branch, prTint(state, checks).word]
    .filter(Boolean)
    .join(" · ");
}

/** One PR of the switcher: a tab whose label is the PR's number, tinted by its
 *  state/CI exactly as the multi-repo strip tints it. Only one chip is in the
 *  tab order (`tabbable`, roving tabindex); the arrow keys are the container's. */
export function PrChip({
  entry,
  selected,
  tabbable,
  disabledReason,
  buttonRef,
  onSelect,
  onKeyDown,
}: {
  entry: PrSetEntry;
  selected: boolean;
  /** The tablist's one tab stop: the selected chip, or the first while none is. */
  tabbable: boolean;
  /** Why switching is unavailable here (a host without the op), or null. */
  disabledReason: string | null;
  buttonRef: Ref<HTMLButtonElement>;
  onSelect: () => void;
  onKeyDown: (e: KeyboardEvent<HTMLButtonElement>) => void;
}) {
  const { state, checks } = entry;
  const { variant } = prTint(state, checks);
  const summary = prSummary(entry);
  return (
    <button
      ref={buttonRef}
      type="button"
      role="tab"
      aria-selected={selected}
      tabIndex={tabbable ? 0 : -1}
      className="git-pr-switch-chip"
      // A plain `title`, not the app's `data-tip`: a disabled button gets no
      // pointer events in the WebView, and the reason is the one thing to say.
      title={disabledReason ? `${summary}\n${disabledReason}` : summary}
      aria-label={summary}
      // The tab stop stays enabled so the tablist keeps a focus stop.
      disabled={disabledReason != null && !tabbable}
      onClick={selected ? undefined : onSelect}
      onKeyDown={onKeyDown}
    >
      <Badge variant={variant}>
        <Icon name={state.state === "merged" ? "merge" : "pr"} size={10} />#{state.number}
      </Badge>
    </button>
  );
}
