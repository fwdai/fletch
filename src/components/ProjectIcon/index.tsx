/** A project's mark: a monochrome rounded square with the first letter of its
 *  name. Radius and letter scale with `size`, so it keeps one silhouette from
 *  the title bar to the page header. */
export function ProjectIcon({ name, size = 16 }: { name: string; size?: number }) {
  return (
    <span
      className="proj-icon iflex-center"
      aria-hidden
      style={{
        width: size,
        height: size,
        borderRadius: Math.round(size * 0.28),
        fontSize: Math.round(size * 0.6),
      }}
    >
      {[...name.trim()][0]?.toUpperCase()}
    </span>
  );
}
