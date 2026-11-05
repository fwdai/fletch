import { useAppStore } from "@/store";
import { PaletteDialog } from "./PaletteDialog";

/** The command palette (⌘K), mounted at the app root like every other
 *  overlay. The store says whether it is open; the dialog, mounted fresh
 *  each time, owns everything else. See `items.ts` for what it offers. */
export function CommandPalette() {
  const open = useAppStore((s) => s.paletteOpen);
  if (!open) return null;
  return <PaletteDialog />;
}
