// Single-style stroked line icons on a 16x16 grid, 1.5 stroke, round caps.
// Ported from the design prototype (icons.jsx plus ui.jsx's FM_EXTRA_ICONS) so
// mobile carries no icon dependency.

import type { ReactElement } from "react";

export const ICON_PATHS = {
  dot: <circle cx="8" cy="8" r="1.5" fill="currentColor" stroke="none" />,
  chevR: <path d="M6 3l5 5-5 5" />,
  chevD: <path d="M3 6l5 5 5-5" />,
  chevU: <path d="M3 10l5-5 5 5" />,
  chevL: <path d="M10 3L5 8l5 5" />,
  close: (
    <>
      <path d="M3.5 3.5l9 9" />
      <path d="M12.5 3.5l-9 9" />
    </>
  ),
  plus: (
    <>
      <path d="M8 3v10" />
      <path d="M3 8h10" />
    </>
  ),
  minus: <path d="M3 8h10" />,
  check: <path d="M3 8.5L6.5 12 13 5" />,
  more: (
    <>
      <circle cx="3.5" cy="8" r=".7" fill="currentColor" stroke="none" />
      <circle cx="8" cy="8" r=".7" fill="currentColor" stroke="none" />
      <circle cx="12.5" cy="8" r=".7" fill="currentColor" stroke="none" />
    </>
  ),
  archive: (
    <>
      <path d="M2.5 5h11v1.5a.5.5 0 0 1-.5.5H3a.5.5 0 0 1-.5-.5V5z" />
      <path d="M3.5 7v5.5a.5.5 0 0 0 .5.5h8a.5.5 0 0 0 .5-.5V7" />
      <path d="M6.5 9.5h3" />
    </>
  ),
  search: (
    <>
      <circle cx="7" cy="7" r="4" />
      <path d="M10 10l3 3" />
    </>
  ),
  refresh: (
    <>
      <path d="M13 4v3h-3" />
      <path d="M13 7a5 5 0 1 0-1.5 4" />
    </>
  ),
  settings: (
    <>
      <circle cx="8" cy="8" r="2" />
      <path d="M8 1v2M8 13v2M15 8h-2M3 8H1M12.95 3.05l-1.42 1.42M4.47 11.53l-1.42 1.42M12.95 12.95l-1.42-1.42M4.47 4.47L3.05 3.05" />
    </>
  ),
  sun: (
    <>
      <circle cx="8" cy="8" r="3" />
      <path d="M8 1.5v1.5M8 13v1.5M1.5 8H3M13 8h1.5M3.4 3.4l1 1M11.6 11.6l1 1M3.4 12.6l1-1M11.6 4.4l1-1" />
    </>
  ),
  moon: <path d="M13 9.5A5 5 0 1 1 6.5 3a4 4 0 0 0 6.5 6.5z" />,
  folder: (
    <path d="M2 5a1 1 0 0 1 1-1h3l1.5 1.5H13a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V5z" />
  ),
  file: (
    <>
      <path d="M4 2h5l3 3v9a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1z" />
      <path d="M9 2v3h3" />
    </>
  ),
  code: (
    <>
      <path d="M5 5L2 8l3 3" />
      <path d="M11 5l3 3-3 3" />
      <path d="M9.5 4l-3 8" />
    </>
  ),
  terminal: (
    <>
      <rect x="2" y="3" width="12" height="10" rx="1.5" />
      <path d="M5 7l2 1.5L5 10" />
      <path d="M9 11h3" />
    </>
  ),
  diff: (
    <>
      <path d="M5 2v8a2 2 0 0 0 2 2h3" />
      <circle cx="5" cy="4" r="1.5" />
      <circle cx="11" cy="12" r="1.5" />
    </>
  ),
  branch: (
    <>
      <circle cx="4" cy="3.5" r="1.5" />
      <circle cx="4" cy="12.5" r="1.5" />
      <circle cx="12" cy="6" r="1.5" />
      <path d="M4 5v6" />
      <path d="M12 7.5v1A2 2 0 0 1 10 10.5H6" />
    </>
  ),
  commit: (
    <>
      <circle cx="8" cy="8" r="2.5" />
      <path d="M2 8h3.5M10.5 8H14" />
    </>
  ),
  merge: (
    <>
      <circle cx="4" cy="3.5" r="1.5" />
      <circle cx="4" cy="12.5" r="1.5" />
      <circle cx="12" cy="12.5" r="1.5" />
      <path d="M4 5v6" />
      <path d="M4 7a4 4 0 0 0 4 4h2.5" />
    </>
  ),
  pr: (
    <>
      <circle cx="4" cy="4" r="1.5" />
      <circle cx="4" cy="12" r="1.5" />
      <circle cx="12" cy="12" r="1.5" />
      <path d="M4 5.5v5" />
      <path d="M12 10.5V7a3 3 0 0 0-3-3H6.5" />
      <path d="M8 2.5L6.5 4 8 5.5" />
    </>
  ),
  push: (
    <>
      <path d="M8 12V3" />
      <path d="M4.5 6.5L8 3l3.5 3.5" />
    </>
  ),
  github: (
    <path
      d="M8 1.5a6.5 6.5 0 0 0-2.1 12.7c.3 0 .4-.1.4-.3v-1.2c-1.8.4-2.2-.8-2.2-.8-.3-.7-.7-.9-.7-.9-.6-.4 0-.4 0-.4.6 0 1 .6 1 .6.6 1 1.5.7 1.9.6.1-.4.2-.7.4-.9-1.4-.2-2.9-.7-2.9-3.2 0-.7.3-1.3.7-1.7-.1-.2-.3-.9.1-1.8 0 0 .6-.2 1.8.6.5-.1 1.1-.2 1.6-.2.6 0 1.1.1 1.6.2 1.2-.8 1.8-.6 1.8-.6.3.9.1 1.6.1 1.8.4.4.7 1 .7 1.7 0 2.5-1.5 3-2.9 3.2.2.2.4.6.4 1.2v1.8c0 .2.1.4.4.3A6.5 6.5 0 0 0 8 1.5z"
      fill="currentColor"
      stroke="none"
    />
  ),
  play: <path d="M5 3v10l8-5z" fill="currentColor" />,
  stop: <rect x="4" y="4" width="8" height="8" rx="1" fill="currentColor" stroke="none" />,
  send: <path d="M14 2L2 7l5 2 2 5z" />,
  copy: (
    <>
      <rect x="5" y="5" width="8" height="8" rx="1.5" />
      <path d="M3 11V4a1 1 0 0 1 1-1h7" />
    </>
  ),
  external: (
    <>
      <path d="M9 3h4v4" />
      <path d="M13 3L7.5 8.5" />
      <path d="M11 9v3.5a.5.5 0 0 1-.5.5h-7a.5.5 0 0 1-.5-.5v-7a.5.5 0 0 1 .5-.5H7" />
    </>
  ),
  clock: (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M8 4.5V8l2 1.5" />
    </>
  ),
  arrowUp: (
    <>
      <path d="M8 13V3" />
      <path d="M4 7l4-4 4 4" />
    </>
  ),
  arrowR: (
    <>
      <path d="M3 8h10" />
      <path d="M9 4l4 4-4 4" />
    </>
  ),
  arrowLeft: (
    <>
      <path d="M13 8H3" />
      <path d="M7 4L3 8l4 4" />
    </>
  ),
  edit: <path d="M11 2.5l2.5 2.5L5 13.5H2.5V11z" />,
  trash: (
    <>
      <path d="M3 5h10" />
      <path d="M5 5V3a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
      <path d="M4 5l.5 8a1 1 0 0 0 1 1h5a1 1 0 0 0 1-1L12 5" />
    </>
  ),
  wrench: (
    <path d="M10 2a3 3 0 0 0-2.6 4.5L2 12v2h2l5.5-5.4A3 3 0 1 0 13 5l-2 2-2-2 2-2A3 3 0 0 0 10 2z" />
  ),
  agent: (
    <>
      <rect x="3.5" y="5" width="9" height="8" rx="2.2" />
      <path d="M8 2.5V5" />
      <circle cx="6" cy="9" r=".9" fill="currentColor" stroke="none" />
      <circle cx="10" cy="9" r=".9" fill="currentColor" stroke="none" />
    </>
  ),
  alert: (
    <>
      <path d="M8 2.5l6 10.5H2z" />
      <path d="M8 6.5v3" />
      <circle cx="8" cy="11.5" r=".6" fill="currentColor" stroke="none" />
    </>
  ),
  laptop: (
    <>
      <rect x="3" y="3.5" width="10" height="7" rx="1.2" />
      <path d="M1.5 12.5h13" />
    </>
  ),
  phone: (
    <>
      <rect x="4.5" y="1.5" width="7" height="13" rx="1.5" />
      <path d="M7 12.5h2" />
    </>
  ),
  lock: (
    <>
      <rect x="3.5" y="7" width="9" height="7" rx="1.5" />
      <path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2" />
    </>
  ),
  checkCircle: (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M5.2 8.3l2 2 3.8-4.2" />
    </>
  ),
  question: (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M6.2 6.3a1.9 1.9 0 1 1 2.6 1.8c-.6.3-.8.7-.8 1.3" />
      <circle cx="8" cy="11.6" r=".6" fill="currentColor" stroke="none" />
    </>
  ),
  star: <path d="M8 1.5v13M1.5 8h13M3.4 3.4l9.2 9.2M12.6 3.4l-9.2 9.2" />,
  task: (
    <>
      <rect x="2.5" y="2.5" width="11" height="11" rx="2" />
      <path d="M5 8l2 2 4-4" />
    </>
  ),
  hand: (
    <>
      <path d="M5 8.5V4a1 1 0 0 1 2 0v4M7 7.5V3a1 1 0 0 1 2 0v4.5M9 7.5V3.8a1 1 0 0 1 2 0v4.7" />
      <path d="M11 8.5a1 1 0 0 1 2 .3V11a4 4 0 0 1-4 4H8a4 4 0 0 1-3.4-1.9L3 10.6a1 1 0 0 1 1.6-1.2L5 10V8.5" />
    </>
  ),
  fetch: (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M2 8h12M8 2c2 2 2 10 0 12" />
      <path d="M6 6l2-2 2 2" />
    </>
  ),
  thinking: (
    <>
      <circle cx="4.5" cy="8" r="1" fill="currentColor" stroke="none" />
      <circle cx="8" cy="8" r="1" fill="currentColor" stroke="none" />
      <circle cx="11.5" cy="8" r="1" fill="currentColor" stroke="none" />
    </>
  ),
  mic: (
    <>
      <rect x="6" y="1.5" width="4" height="8" rx="2" />
      <path d="M3.5 7.5a4.5 4.5 0 0 0 9 0" />
      <path d="M8 12v2.5M5.5 14.5h5" />
    </>
  ),
  micOff: (
    <>
      <path d="M6 6v1.5a2 2 0 0 0 3.4 1.4" />
      <path d="M10 7.5V3.5a2 2 0 0 0-4 0v.5" />
      <path d="M3.5 7.5a4.5 4.5 0 0 0 7.2 3.6M12.5 7.5a4.5 4.5 0 0 1-.4 1.8" />
      <path d="M8 12v2.5M5.5 14.5h5" />
      <path d="M2.5 2.5l11 11" />
    </>
  ),
} satisfies Record<string, ReactElement>;

export type IconName = keyof typeof ICON_PATHS;
