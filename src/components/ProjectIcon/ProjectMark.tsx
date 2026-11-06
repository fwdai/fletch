import type { ProjectIconData } from "@/api/types/agent";
import { Icon } from "@/components/Icon";
import { glyphIcon } from "./glyphs";

/** A project's mark, the one silhouette every placement draws: a rounded
 *  square holding its image, a built-in glyph, or — with neither — the first
 *  letter of its name, monochrome. Radius, glyph and letter scale with `size`.
 *  An SVG goes through `<img>`, never inlined, so nothing in it runs. Shared
 *  with the phone, so it fetches nothing itself. */
export function ProjectMark({
  name,
  icon,
  size = 16,
}: {
  name: string;
  icon: ProjectIconData | null;
  size?: number;
}) {
  const glyph = icon && "glyph" in icon ? glyphIcon(icon.glyph) : undefined;
  return (
    <span
      className="proj-icon"
      aria-hidden
      style={{
        width: size,
        height: size,
        borderRadius: Math.round(size * 0.28),
        fontSize: Math.round(size * 0.6),
      }}
    >
      {icon && "data" in icon ? (
        <img src={`data:${icon.mime};base64,${icon.data}`} alt="" draggable={false} />
      ) : glyph ? (
        <Icon name={glyph} size={Math.round(size * 0.68)} />
      ) : (
        [...name.trim()][0]?.toUpperCase()
      )}
    </span>
  );
}
