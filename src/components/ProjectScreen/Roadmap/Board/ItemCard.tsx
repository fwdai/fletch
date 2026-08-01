import { Icon, type IconName } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { type ItemSource, type RoadmapItem, SIZE_HINT } from "../types";

/** Where the item came from, as a one-glyph tag. */
const SOURCE: Record<ItemSource, { icon: IconName; tip: string }> = {
  pm: { icon: "sparkle", tip: "Written here with the PM agent" },
  linear: { icon: "layers", tip: "From Linear" },
  github: { icon: "github", tip: "From GitHub" },
};

interface Props {
  item: RoadmapItem;
  /** The project's primary repo — where "Send to an agent" opens the draft. */
  repoPath: string;
  /** A proposed row — real only once the user accepts the proposal. */
  ghost?: boolean;
  open: boolean;
  onToggle: () => void;
  /** Ring the row: it was just jumped to, or a pending proposal moves it. */
  focused?: boolean;
  /** Lets the board scroll a focused row into view. */
  cardRef?: (el: HTMLDivElement | null) => void;
}

/** The item as a starting prompt for an agent: what to build, why, and what
 *  "done" means. Prefills the new draft's composer so the user reviews and
 *  launches rather than retyping the row. */
function briefFor(item: RoadmapItem): string {
  const lines = [`${item.code}: ${item.title}`, "", item.why];
  if (item.accept?.length) lines.push("", "Done when:", ...item.accept.map((a) => `- ${a}`));
  return lines.join("\n");
}

/** One roadmap row: a click-to-expand header line (code, title, size, source)
 *  over the rationale, acceptance criteria and dependencies. */
export function ItemCard({ item, repoPath, ghost, open, onToggle, focused, cardRef }: Props) {
  const createDraft = useAppStore((s) => s.createDraft);
  const closeProjectScreen = useAppStore((s) => s.closeProjectScreen);
  const source = SOURCE[item.source];
  const cls = [
    "rm-item",
    ghost ? "ghost" : "",
    open ? "open" : "",
    item.justAdded ? "landed" : "",
    focused ? "focus" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div ref={cardRef} className={cls}>
      <button
        type="button"
        className="rm-item-h flex-center"
        onClick={onToggle}
        aria-expanded={open}
      >
        <span className="rm-code mono text-xs">{item.code}</span>
        <span className="rm-title text-sm truncate">{item.title}</span>
        {item.epic && <span className="rm-epic text-xs">{item.epic}</span>}
        {item.status === "active" && item.agent && (
          <span className="rm-live iflex-center mono text-xs">
            <span className="rm-pearl" />
            {item.agent}
          </span>
        )}
        <span
          className={`rm-size iflex-center mono text-xs s-${item.size}`}
          title={SIZE_HINT[item.size]}
        >
          {item.size}
        </span>
        <span className={`rm-src iflex-center src-${item.source}`} title={source.tip}>
          <Icon name={source.icon} size={10} />
        </span>
        <Icon name="chevD" size={10} className="rm-chev" />
      </button>

      {open && (
        <div className="rm-item-body">
          <p className="rm-why text-sm">{item.why}</p>
          {item.accept && (
            <ul className="rm-accept text-sm">
              {item.accept.map((a) => (
                <li key={a}>{a}</li>
              ))}
            </ul>
          )}
          <div className="rm-item-foot flex-center">
            <span className="rm-area mono text-xs">{item.area}</span>
            {item.deps?.map((d) => (
              <span key={d} className="rm-dep iflex-center mono text-xs">
                <Icon name="arrowR" size={9} />
                after {d}
              </span>
            ))}
            <span className="grow" />
            {/* A proposed row has nothing to send an agent at yet. */}
            {!ghost && (
              <Button
                variant="outline"
                size="sm"
                onClick={async () => {
                  const draftId = await createDraft(repoPath, briefFor(item));
                  // The draft lives in the workspace, which this page covers —
                  // but stay put if it couldn't be created, so the user isn't
                  // dropped somewhere else to read the error.
                  if (draftId) closeProjectScreen();
                }}
              >
                <Icon name="zap" size={11} /> Send to an agent
              </Button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
