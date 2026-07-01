/** Fletch brand mark: a double chevron (»), echoing the app icon. The leading
 *  chevron inherits `currentColor` so it tracks the surrounding text color (and
 *  adapts across themes); the trailing chevron carries the brand accent. Sized
 *  by the consumer via font-size / width+height. */
export function FletchMark({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      viewBox="0 0 24 24"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden="true"
      focusable="false"
    >
      <path
        d="M5 6L11 12L5 18"
        stroke="currentColor"
        strokeWidth="3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M14 6L20 12L14 18"
        style={{ stroke: "var(--accent)" }}
        strokeWidth="3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
