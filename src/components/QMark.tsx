/** Quorum brand "Q" mark. The ring inherits `currentColor` so it adapts
 *  to the surrounding text color (works in both themes); the tail keeps
 *  the brand orange. Sized via font-size / CSS height by the consumer. */
export function QMark({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      viewBox="0 0 584 585"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden="true"
      focusable="false"
    >
      <path
        d="M441.142 548.642L529.892 459.892L441.142 371.142"
        stroke="url(#qmark_tail)"
        strokeWidth="72"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M519.313 312.563C528.041 313.946 536.292 307.992 537.104 299.193C541.583 250.648 531.512 201.73 507.986 158.755C481.892 111.09 440.721 73.4286 390.926 51.6739C341.13 29.9191 285.524 25.3003 232.821 38.5412C180.119 51.7821 133.299 82.1344 99.6987 124.841C66.0984 167.549 47.6169 220.197 47.1506 274.535C46.6843 328.873 64.2595 381.831 97.1219 425.108C129.984 468.386 176.277 499.537 228.744 513.68C276.049 526.432 325.962 524.709 372.09 508.931C380.451 506.071 384.296 496.65 380.898 488.493L356.171 429.14C352.773 420.983 343.421 417.215 334.947 419.72C308.611 427.508 280.525 427.903 253.808 420.701C221.921 412.106 193.787 393.173 173.815 366.872C153.843 340.57 143.162 308.385 143.445 275.361C143.728 242.337 154.961 210.341 175.381 184.385C195.802 158.43 224.256 139.984 256.286 131.937C288.316 123.89 322.11 126.697 352.374 139.918C382.637 153.139 407.658 176.028 423.516 204.996C436.803 229.268 443.029 256.658 441.658 284.087C441.217 292.913 447.08 301.116 455.807 302.499L519.313 312.563Z"
        fill="currentColor"
      />
      <defs>
        <linearGradient id="qmark_tail" x1="485.517" y1="371.142" x2="485.517" y2="548.642" gradientUnits="userSpaceOnUse">
          <stop stopColor="#FD9E64" />
          <stop offset="1" stopColor="#E58B55" />
        </linearGradient>
      </defs>
    </svg>
  );
}
