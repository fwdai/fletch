import { useShortcutKeys } from "@/util/shortcuts";
import type { PageId } from "./types";

/** The key legend. Reads differently on a page: Enter picks and Esc backs
 *  out, rather than running and closing. */
export function PaletteFooter({ page }: { page: PageId }) {
  const root = page === "root";
  const chord = useShortcutKeys("palette");
  return (
    <div className="cp-foot flex-center text-xs">
      <span>
        <kbd className="kbd">↑↓</kbd>
        move
      </span>
      <span>
        <kbd className="kbd">↵</kbd>
        {root ? "run" : "pick"}
      </span>
      <span>
        <kbd className="kbd">esc</kbd>
        {root ? "close" : "back"}
      </span>
      {chord && (
        <span className="cp-foot-r">
          <kbd className="kbd">{chord}</kbd>
          toggle
        </span>
      )}
    </div>
  );
}
