export function Wordmark() {
  return (
    <div className="wordmark">
      <svg
        width="18"
        height="18"
        viewBox="0 0 16 16"
        fill="none"
        stroke="var(--accent)"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <path d="M2.5 3.5l4.5 4.5-4.5 4.5M8.5 3.5L13 8l-4.5 4.5" />
      </svg>
      fletch
      <span className="beta">BETA</span>
    </div>
  );
}
