// Single-style stroked line icons + landmark glyphs.
// 16x16 grid, 1.5 stroke, round caps/joins.

import type { CSSProperties, ReactNode } from "react";
import { createElement, Fragment } from "react";
import { LANDMARK_GLYPHS } from "../data/landmarks";

const e = createElement;

export type IconName = keyof typeof ICON_PATHS;

const ICON_PATHS = {
  dot: e("circle", { cx: 8, cy: 8, r: 1.5, fill: "currentColor", stroke: "none" }),
  chevR: e("path", { d: "M6 3l5 5-5 5" }),
  chevD: e("path", { d: "M3 6l5 5 5-5" }),
  chevU: e("path", { d: "M3 10l5-5 5 5" }),
  chevL: e("path", { d: "M10 3L5 8l5 5" }),
  close: e(Fragment, null, e("path", { d: "M3.5 3.5l9 9" }), e("path", { d: "M12.5 3.5l-9 9" })),
  plus: e(Fragment, null, e("path", { d: "M8 3v10" }), e("path", { d: "M3 8h10" })),
  minus: e("path", { d: "M3 8h10" }),
  check: e("path", { d: "M3 8.5L6.5 12 13 5" }),
  more: e(
    Fragment,
    null,
    e("circle", { cx: 3.5, cy: 8, r: 0.7, fill: "currentColor", stroke: "none" }),
    e("circle", { cx: 8, cy: 8, r: 0.7, fill: "currentColor", stroke: "none" }),
    e("circle", { cx: 12.5, cy: 8, r: 0.7, fill: "currentColor", stroke: "none" }),
  ),
  search: e(Fragment, null, e("circle", { cx: 7, cy: 7, r: 4 }), e("path", { d: "M10 10l3 3" })),
  refresh: e(Fragment, null, e("path", { d: "M13 4v3h-3" }), e("path", { d: "M13 7a5 5 0 1 0-1.5 4" })),
  settings: e(
    Fragment,
    null,
    e("circle", { cx: 8, cy: 8, r: 2 }),
    e("path", {
      d: "M8 1v2M8 13v2M15 8h-2M3 8H1M12.95 3.05l-1.42 1.42M4.47 11.53l-1.42 1.42M12.95 12.95l-1.42-1.42M4.47 4.47L3.05 3.05",
    }),
  ),
  user: e(
    Fragment,
    null,
    e("circle", { cx: 8, cy: 5.5, r: 2.5 }),
    e("path", { d: "M3 14c.5-2.5 2.7-4 5-4s4.5 1.5 5 4" }),
  ),
  folder: e("path", {
    d: "M2 5a1 1 0 0 1 1-1h3l1.5 1.5H13a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V5z",
  }),
  file: e(
    Fragment,
    null,
    e("path", { d: "M4 2h5l3 3v9a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1z" }),
    e("path", { d: "M9 2v3h3" }),
  ),
  code: e(
    Fragment,
    null,
    e("path", { d: "M5 5L2 8l3 3" }),
    e("path", { d: "M11 5l3 3-3 3" }),
    e("path", { d: "M9.5 4l-3 8" }),
  ),
  terminal: e(
    Fragment,
    null,
    e("rect", { x: 2, y: 3, width: 12, height: 10, rx: 1.5 }),
    e("path", { d: "M5 7l2 1.5L5 10" }),
    e("path", { d: "M9 11h3" }),
  ),
  branch: e(
    Fragment,
    null,
    e("circle", { cx: 4, cy: 3.5, r: 1.5 }),
    e("circle", { cx: 4, cy: 12.5, r: 1.5 }),
    e("circle", { cx: 12, cy: 6, r: 1.5 }),
    e("path", { d: "M4 5v6" }),
    e("path", { d: "M12 7.5v1A2 2 0 0 1 10 10.5H6" }),
  ),
  commit: e(
    Fragment,
    null,
    e("circle", { cx: 8, cy: 8, r: 2.5 }),
    e("path", { d: "M2 8h3.5M10.5 8H14" }),
  ),
  merge: e(
    Fragment,
    null,
    e("circle", { cx: 4, cy: 3.5, r: 1.5 }),
    e("circle", { cx: 4, cy: 12.5, r: 1.5 }),
    e("circle", { cx: 12, cy: 12.5, r: 1.5 }),
    e("path", { d: "M4 5v6" }),
    e("path", { d: "M4 7a4 4 0 0 0 4 4h2.5" }),
  ),
  pr: e(
    Fragment,
    null,
    e("circle", { cx: 4, cy: 4, r: 1.5 }),
    e("circle", { cx: 4, cy: 12, r: 1.5 }),
    e("circle", { cx: 12, cy: 12, r: 1.5 }),
    e("path", { d: "M4 5.5v5" }),
    e("path", { d: "M12 10.5V7a3 3 0 0 0-3-3H6.5" }),
    e("path", { d: "M8 2.5L6.5 4 8 5.5" }),
  ),
  push: e(Fragment, null, e("path", { d: "M8 12V3" }), e("path", { d: "M4.5 6.5L8 3l3.5 3.5" })),
  github: e("path", {
    d: "M8 1.5a6.5 6.5 0 0 0-2.1 12.7c.3 0 .4-.1.4-.3v-1.2c-1.8.4-2.2-.8-2.2-.8-.3-.7-.7-.9-.7-.9-.6-.4 0-.4 0-.4.6 0 1 .6 1 .6.6 1 1.5.7 1.9.6.1-.4.2-.7.4-.9-1.4-.2-2.9-.7-2.9-3.2 0-.7.3-1.3.7-1.7-.1-.2-.3-.9.1-1.8 0 0 .6-.2 1.8.6.5-.1 1.1-.2 1.6-.2.6 0 1.1.1 1.6.2 1.2-.8 1.8-.6 1.8-.6.3.9.1 1.6.1 1.8.4.4.7 1 .7 1.7 0 2.5-1.5 3-2.9 3.2.2.2.4.6.4 1.2v1.8c0 .2.1.4.4.3A6.5 6.5 0 0 0 8 1.5z",
    fill: "currentColor",
    stroke: "none",
  }),
  play: e("path", { d: "M5 3v10l8-5z", fill: "currentColor" }),
  pause: e(
    Fragment,
    null,
    e("rect", { x: 4, y: 3, width: 3, height: 10, rx: 1, fill: "currentColor", stroke: "none" }),
    e("rect", { x: 9, y: 3, width: 3, height: 10, rx: 1, fill: "currentColor", stroke: "none" }),
  ),
  stop: e("rect", { x: 4, y: 4, width: 8, height: 8, rx: 1, fill: "currentColor", stroke: "none" }),
  attach: e("path", { d: "M10 4.5L5 9.5a2.5 2.5 0 0 0 3.5 3.5L13.5 8a4 4 0 1 0-5.7-5.7L3 7" }),
  external: e(
    Fragment,
    null,
    e("path", { d: "M9 3h4v4" }),
    e("path", { d: "M13 3L7.5 8.5" }),
    e("path", { d: "M11 9v3.5a.5.5 0 0 1-.5.5h-7a.5.5 0 0 1-.5-.5v-7a.5.5 0 0 1 .5-.5H7" }),
  ),
  arrowUp: e(Fragment, null, e("path", { d: "M8 13V3" }), e("path", { d: "M4 7l4-4 4 4" })),
  sidebarL: e(
    Fragment,
    null,
    e("rect", { x: 2, y: 3, width: 12, height: 10, rx: 1.5 }),
    e("path", { d: "M6 3v10" }),
  ),
  sidebarR: e(
    Fragment,
    null,
    e("rect", { x: 2, y: 3, width: 12, height: 10, rx: 1.5 }),
    e("path", { d: "M10 3v10" }),
  ),
  sparkle: e("path", {
    d: "M8 2v3M8 11v3M2 8h3M11 8h3M3.5 3.5l2 2M10.5 10.5l2 2M3.5 12.5l2-2M10.5 5.5l2-2",
  }),
  inbox: e(
    Fragment,
    null,
    e("path", { d: "M2 8l1.5-5h9L14 8" }),
    e("path", { d: "M2 8v4a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V8h-3l-1 2H6L5 8H2z" }),
  ),
  edit: e("path", { d: "M11 2.5l2.5 2.5L5 13.5H2.5V11z" }),
  trash: e(
    Fragment,
    null,
    e("path", { d: "M3 5h10" }),
    e("path", { d: "M5 5V3a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" }),
    e("path", { d: "M4 5l.5 8a1 1 0 0 0 1 1h5a1 1 0 0 0 1-1L12 5" }),
  ),
  thinking: e(
    Fragment,
    null,
    e("circle", { cx: 4.5, cy: 8, r: 1, fill: "currentColor", stroke: "none" }),
    e("circle", { cx: 8, cy: 8, r: 1, fill: "currentColor", stroke: "none" }),
    e("circle", { cx: 11.5, cy: 8, r: 1, fill: "currentColor", stroke: "none" }),
  ),
  wrench: e("path", {
    d: "M10 2a3 3 0 0 0-2.6 4.5L2 12v2h2l5.5-5.4A3 3 0 1 0 13 5l-2 2-2-2 2-2A3 3 0 0 0 10 2z",
  }),
};

interface IconProps {
  name: keyof typeof ICON_PATHS;
  size?: number;
  className?: string;
  strokeWidth?: number;
  style?: CSSProperties;
}

export function Icon({ name, size = 14, className, strokeWidth = 1.5, style }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      style={style}
    >
      {ICON_PATHS[name] ?? ICON_PATHS.dot}
    </svg>
  );
}

interface LandmarkGlyphProps {
  name: string;
  size?: number;
  strokeWidth?: number;
  className?: string;
  style?: CSSProperties;
}

const FALLBACK_GLYPH: ReactNode = e("path", { d: "M2 13 L 6 7 L 9 10 L 14 5" });

export function LandmarkGlyph({
  name,
  size = 16,
  strokeWidth = 1.4,
  className,
  style,
}: LandmarkGlyphProps) {
  const glyph = LANDMARK_GLYPHS[name] ?? FALLBACK_GLYPH;
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      style={style}
    >
      {glyph}
    </svg>
  );
}
