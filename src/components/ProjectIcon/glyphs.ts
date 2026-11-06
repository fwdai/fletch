import type { IconName } from "@/components/Icon";

/** The built-in marks a project may wear instead of an image or its initial
 *  (`builtin:<id>`). The ids are the host's (`icon_detect::GLYPHS`). */
export const PROJECT_GLYPHS: { id: string; label: string; icon: IconName }[] = [
  { id: "web", label: "Web", icon: "globe" },
  { id: "desktop", label: "Desktop", icon: "laptop" },
  { id: "mobile", label: "Mobile", icon: "phone" },
  { id: "cli", label: "CLI", icon: "terminal" },
  { id: "library", label: "Library", icon: "blocks" },
  { id: "docs", label: "Docs", icon: "file" },
];

export function glyphIcon(id: string): IconName | undefined {
  return PROJECT_GLYPHS.find((g) => g.id === id)?.icon;
}
