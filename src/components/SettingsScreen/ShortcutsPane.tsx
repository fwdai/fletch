import { Fragment } from "react";
import { type Combo, formatCombo, visibleShortcutGroups } from "@/util/keymap";
import { SetGroup, SetHead, SetRow } from "./primitives";

/** The chords for one row: alternatives separated by a slash. */
function Keys({ combos }: { combos: Combo[] }) {
  return combos.map((combo, i) => (
    <Fragment key={combo}>
      {i > 0 && <span className="set-kbd-sep text-xs">/</span>}
      <kbd className="kbd set-kbd">{formatCombo(combo)}</kbd>
    </Fragment>
  ));
}

/** Settings › Interface › Shortcuts: every key binding, read from the same map
 *  the global handler dispatches from (`util/keymap.ts`). A reference for now;
 *  rebinding is not offered yet. */
export function ShortcutsPane() {
  const groups = visibleShortcutGroups();
  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Shortcuts"
        title="Keyboard shortcuts"
        desc="Every key binding in Fletch, grouped by where it applies. The first three groups work anywhere in the window; the rest belong to the surface named. Bindings can't be changed yet."
      />

      {groups.map((g, i) => (
        <SetGroup key={g.label} label={g.label} last={i === groups.length - 1}>
          {g.items.map((it) => (
            <SetRow key={it.id} title={it.label} sub={it.description}>
              <Keys combos={it.combos} />
            </SetRow>
          ))}
        </SetGroup>
      ))}
    </div>
  );
}
