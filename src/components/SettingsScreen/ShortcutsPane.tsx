import { Fragment, useEffect, useState } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import {
  bindingProblem,
  type Combo,
  comboFromEvent,
  effectiveCombos,
  formatCombo,
  REBINDABLE_IDS,
  type Shortcut,
  visibleShortcutGroups,
} from "@/util/keymap";
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

/** A rebindable row's control: the keycaps, which record a new chord when
 *  clicked, and a reset once the row is off its defaults. Recording captures
 *  the next keydown ahead of every other listener (so ⌘K rebinds rather than
 *  opening search), Esc backs out, and a chord the map refuses stays in the
 *  row with the reason until a good one lands. The pane owns which row is
 *  recording, so clicking another row hands the listener over rather than
 *  stacking a second one. */
function Recorder({
  shortcut,
  combos,
  recording,
  onRecord,
}: {
  shortcut: Shortcut;
  combos: Combo[];
  recording: boolean;
  onRecord: (on: boolean) => void;
}) {
  const overrides = useAppStore((s) => s.shortcutOverrides);
  const setShortcut = useAppStore((s) => s.setShortcut);
  const resetShortcut = useAppStore((s) => s.resetShortcut);
  const [problem, setProblem] = useState<string | null>(null);
  const overridden = shortcut.id in overrides;

  useEffect(() => {
    if (!recording) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopImmediatePropagation();
      if (e.key === "Escape") {
        onRecord(false);
        return;
      }
      const combo = comboFromEvent(e);
      if (!combo) return;
      const why = bindingProblem(shortcut.id, combo, overrides);
      if (why) {
        setProblem(why);
        return;
      }
      onRecord(false);
      if (!combos.includes(combo)) setShortcut(shortcut.id, [combo]);
    };
    const stop = () => onRecord(false);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("blur", stop);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("blur", stop);
    };
  }, [recording, onRecord, overrides, combos, shortcut.id, setShortcut]);

  return (
    <>
      {recording && problem && <span className="set-kbd-problem text-xs">{problem}</span>}
      <button
        type="button"
        className={`set-kbd-btn iflex-center ${recording ? "" : "tip"}`}
        data-tip={recording ? undefined : "Click, then press the new keys"}
        aria-label={`Change ${shortcut.label} shortcut`}
        onClick={() => {
          setProblem(null);
          onRecord(true);
        }}
      >
        {recording ? (
          <kbd className="kbd set-kbd set-kbd-rec">Press keys…</kbd>
        ) : (
          <Keys combos={combos} />
        )}
      </button>
      {overridden && !recording && (
        <IconButton
          size="sm"
          tip={`Reset to ${shortcut.combos.map((c) => formatCombo(c)).join(" / ")}`}
          onClick={() => resetShortcut(shortcut.id)}
        >
          <Icon name="refresh" />
        </IconButton>
      )}
    </>
  );
}

/** Settings › Interface › Shortcuts: every key binding, read from the same map
 *  the global handler dispatches from (`util/keymap.ts`). Global rows can be
 *  rebound by clicking their keys; contextual rows are a reference. */
export function ShortcutsPane() {
  const overrides = useAppStore((s) => s.shortcutOverrides);
  const resetAll = useAppStore((s) => s.resetAllShortcuts);
  // At most one row records at a time: the id of the row that is, or null.
  const [recordingId, setRecordingId] = useState<string | null>(null);
  const groups = visibleShortcutGroups();
  const customized = Object.keys(overrides).length > 0;
  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Shortcuts"
        title="Keyboard shortcuts"
        desc="Every key binding in Fletch, grouped by where it applies. The first three groups work anywhere in the window and can be changed: click the keys, then press the new ones. The rest belong to the surface named and are fixed."
        actions={
          customized ? (
            <Button variant="outline" size="sm" onClick={resetAll}>
              Reset all
            </Button>
          ) : undefined
        }
      />

      {groups.map((g, i) => (
        <SetGroup key={g.label} label={g.label} last={i === groups.length - 1}>
          {g.items.map((it) => (
            <SetRow key={it.id} title={it.label} sub={it.description}>
              {REBINDABLE_IDS.has(it.id) ? (
                <Recorder
                  shortcut={it}
                  combos={effectiveCombos(it, overrides)}
                  recording={recordingId === it.id}
                  onRecord={(on) =>
                    setRecordingId((cur) => (on ? it.id : cur === it.id ? null : cur))
                  }
                />
              ) : (
                <Keys combos={it.combos} />
              )}
            </SetRow>
          ))}
        </SetGroup>
      ))}
    </div>
  );
}
