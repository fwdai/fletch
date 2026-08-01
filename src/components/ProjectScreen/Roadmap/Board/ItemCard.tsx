import { Icon, type IconName } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { type BoardItem, type ItemSource, SIZE_HINT } from "../types";

/** Where the item came from, as a one-glyph tag. */
const SOURCE: Record<ItemSource, { icon: IconName; tip: string }> = {
  user: { icon: "user", tip: "Added by hand" },
  pm: { icon: "sparkle", tip: "Written here with the PM agent" },
  linear: { icon: "layers", tip: "From Linear" },
  github: { icon: "github", tip: "From GitHub" },
};

interface Props {
  item: BoardItem;
  /** The project's primary repo — where "Send to an agent" opens the draft. */
  repoPath: string;
  /** A proposed row: on the board, but not on the roadmap until it's accepted. */
  ghost?: boolean;
  open: boolean;
  onToggle: () => void;
  /** Accept the proposal (`proposed → open`). Ghosts only, and not read-only. */
  onAccept?: () => void;
  /** Discard the proposal — the row is deleted. Ghosts only. */
  onDiscard?: () => void;
  /** Ring the row: it was just jumped to, or a pending proposal moves it. */
  focused?: boolean;
  /** Transient highlight for a row that just landed or just moved. */
  landed?: boolean;
  /** Open this item's form. Absent for a ghost (there is no row to edit yet)
   *  and on a read-only board. */
  onEdit?: () => void;
  /** Lets the board scroll a focused row into view. */
  cardRef?: (el: HTMLDivElement | null) => void;
}

/** The item as a starting prompt for an agent: what to build, why, and what
 *  "done" means. Prefills the new draft's composer so the user reviews and
 *  launches rather than retyping the row. */
function briefFor(item: BoardItem): string {
  const lines = [`${item.code}: ${item.title}`, "", item.why];
  if (item.accept?.length) lines.push("", "Done when:", ...item.accept.map((a) => `- ${a}`));
  return lines.join("\n").trim();
}

/** One roadmap row: a click-to-expand header line (code, title, size, source)
 *  over the rationale, acceptance criteria and dependencies. */
export function ItemCard({
  item,
  repoPath,
  ghost,
  open,
  onToggle,
  focused,
  landed,
  onEdit,
  onAccept,
  onDiscard,
  cardRef,
}: Props) {
  const createDraft = useAppStore((s) => s.createDraft);
  const closeProjectScreen = useAppStore((s) => s.closeProjectScreen);
  const source = SOURCE[item.source];
  const cls = [
    "rm-item",
    ghost ? "ghost" : "",
    open ? "open" : "",
    landed ? "landed" : "",
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
        {/* A proposed row already owns its code (the PM quotes it in the chat
            the moment it proposes), and accepting one never renumbers it. */}
        <span className="rm-code mono text-xs">{item.code}</span>
        <span className="rm-title text-sm truncate">{item.title}</span>
        {item.epic && <span className="rm-epic text-xs">{item.epic}</span>}
        {item.status === "active" && item.agent && (
          <span className="rm-live iflex-center mono text-xs">
            <span className="rm-pearl" />
            {item.agent}
          </span>
        )}
        {item.size && (
          <span
            className={`rm-size iflex-center mono text-xs s-${item.size}`}
            title={SIZE_HINT[item.size]}
          >
            {item.size}
          </span>
        )}
        <span className={`rm-src iflex-center src-${item.source}`} title={source.tip}>
          <Icon name={source.icon} size={10} />
        </span>
        <Icon name="chevD" size={10} className="rm-chev" />
      </button>

      {/* The two buttons that decide a proposal's fate. Outside the header
          button (no nesting) and outside the collapsible body, so ruling on a
          ghost never costs an expand — reading it first is what the expand is
          for. */}
      {ghost && (onAccept || onDiscard) && (
        <div className="rm-ghostbar flex-center">
          <span className="rm-ghostbar-l text-xs">Proposed — not on the roadmap yet</span>
          <span className="grow" />
          {onDiscard && (
            <Button variant="ghost" size="sm" onClick={onDiscard}>
              Discard
            </Button>
          )}
          {onAccept && (
            <Button variant="primary" size="sm" onClick={onAccept}>
              <Icon name="check" size={11} /> Accept
            </Button>
          )}
        </div>
      )}

      {open && (
        <div className="rm-item-body">
          {item.why && <p className="rm-why text-sm">{item.why}</p>}
          {item.accept && (
            <ul className="rm-accept text-sm">
              {item.accept.map((a) => (
                <li key={a}>{a}</li>
              ))}
            </ul>
          )}
          <div className="rm-item-foot flex-center">
            {item.area && <span className="rm-area mono text-xs">{item.area}</span>}
            {item.deps?.map((d) => (
              <span key={d} className="rm-dep iflex-center mono text-xs">
                <Icon name="arrowR" size={9} />
                after {d}
              </span>
            ))}
            <span className="grow" />
            {onEdit && (
              <Button variant="ghost" size="sm" onClick={onEdit}>
                <Icon name="edit" size={11} /> Edit
              </Button>
            )}
            {/* A proposed row isn't work anyone has agreed to do — accept it
                first, then send it. */}
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
