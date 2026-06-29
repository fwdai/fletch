import { shortFor } from "./shared";

/** Colored monogram tile standing in for a custom agent's avatar. The hue is
 *  the agent's `color`; the glyph is its name's initials. */
export function Mono({ name, hue, size = 38 }: { name: string; hue: number; size?: number }) {
  return (
    <span
      className="ca-mono iflex-center"
      style={{
        width: size,
        height: size,
        borderRadius: size > 30 ? 9 : 6,
        fontSize: size > 30 ? 13 : 10.5,
        ["--h" as string]: hue,
      }}
    >
      {shortFor(name)}
    </span>
  );
}
