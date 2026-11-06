import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";

/** What a new project's board says instead of three empty horizons.
 *
 *  Three empty groups read as a broken screen; this reads as a starting point.
 *  It names the two ways in — the conversation on the left (the one we want) and
 *  the manual row (the one that's always available) — and nothing else. */
export function EmptyBoard({ onAdd, readOnly }: { onAdd: () => void; readOnly?: boolean }) {
  return (
    <div className="rm-blank">
      <span className="rm-blank-badge iflex-center">
        <Icon name="map" size={18} />
      </span>
      <h3 className="rm-blank-h text-base">Nothing on the roadmap yet</h3>
      <p className="rm-blank-b text-sm">
        Tell the project manager on the left what you want to build. It reads the repo first, then
        proposes items — you decide what lands here.
      </p>
      {!readOnly && (
        <>
          <span className="rm-blank-or text-xs">or</span>
          <Button variant="outline" size="sm" onClick={onAdd}>
            <Icon name="plus" size={11} /> Add an item yourself
          </Button>
        </>
      )}
    </div>
  );
}
