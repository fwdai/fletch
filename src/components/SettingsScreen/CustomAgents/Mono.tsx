import { shortFor } from "./shared";

/** Colored monogram tile standing in for a custom agent's avatar. The hue is
 *  the agent's `color`; the glyph is its name's initials. Corner radius and
 *  glyph size scale with `size`, so every chip — from the 14px sidebar dot to
 *  the 46px editor head — keeps the same proportions and never crowds its edges. */
export function Mono({ name, hue, size = 38 }: { name: string; hue: number; size?: number }) {
  return (
    <span
      className="ca-mono iflex-center"
      style={{
        width: size,
        height: size,
        borderRadius: Math.round(size * 0.24),
        fontSize: Math.round((5 + size * 0.18) * 2) / 2,
        ["--h" as string]: hue,
      }}
    >
      {shortFor(name)}
    </span>
  );
}
