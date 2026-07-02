import { Icon } from "@/components/Icon";
import { editorFace } from "./editors";

/** The little editor tile — the brand mark, a glyph for the terminal, or a
 *  monogram fallback, all on the one premium tile surface (see the CSS).
 *  Always renders, so the launcher never shows an empty square. */
export function EditorTile({ id, size = 18 }: { id: string; size?: number }) {
  const face = editorFace(id);
  const box = { width: size, height: size };
  if (face.logo) {
    return (
      <span className="oe-tile" style={box}>
        <svg
          viewBox="0 0 24 24"
          width={Math.round(size * 0.6)}
          height={Math.round(size * 0.6)}
          fill="currentColor"
          aria-hidden="true"
        >
          <path d={face.logo} />
        </svg>
      </span>
    );
  }
  if (face.icon) {
    return (
      <span className="oe-tile" style={box}>
        <Icon name={face.icon} size={size <= 18 ? 11 : 13} />
      </span>
    );
  }
  return (
    <span className="oe-tile" style={{ ...box, fontSize: size <= 18 ? 8.5 : 10 }}>
      {face.mono}
    </span>
  );
}
