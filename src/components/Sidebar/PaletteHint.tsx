import { useAppStore } from "@/store";
import { useShortcutKeys } from "@/util/shortcuts";

/** A quiet reminder that the command palette exists, and its one way in by
 *  mouse. It lives here because the title bar is kept to a few actions. */
export function PaletteHint() {
  const togglePalette = useAppStore((s) => s.togglePalette);
  const keys = useShortcutKeys("palette");

  return (
    <button className="side-palette-hint flex-center text-sm" onClick={() => togglePalette(true)}>
      {keys && <span className="kbd">{keys}</span>}
      <span>Command palette</span>
    </button>
  );
}
