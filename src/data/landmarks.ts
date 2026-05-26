// Worktree names + tiny location glyphs.

import type { ReactNode } from "react";
import { createElement, Fragment } from "react";

const e = createElement;

export const LANDMARK_GLYPHS: Record<string, ReactNode> = {
  dolomites: e("path", { d: "M1 13 L3.5 9 L5 11 L8 4 L10 8 L11.5 6 L14 11 L15 13" }),
  caspian: e(
    Fragment,
    null,
    e("circle", { cx: 11, cy: 5.5, r: 2.5 }),
    e("path", { d: "M1 11 Q 3 9.5 5 11 T 9 11 T 13 11 M 1 14 Q 3 12.5 5 14 T 9 14 T 13 14 T 15 14" }),
  ),
  yosemite: e(
    Fragment,
    null,
    e("path", { d: "M2 13 Q 4 6 9 5 L 9 13 Z", fill: "currentColor", stroke: "none", opacity: 0.25 }),
    e("path", { d: "M2 13 Q 4 6 9 5 L 9 13 M 9 5 Q 11 8 12 13 L 9 13" }),
  ),
  patagonia: e("path", { d: "M1 13 L 3 10 L 5 11 L 6 4 L 8 9 L 10 3 L 12 8 L 14 11 L 15 13" }),
  hokkaido: e(
    Fragment,
    null,
    e("path", { d: "M2 13 L 8 3 L 14 13" }),
    e("path", { d: "M6 6.5 L 7 7.5 L 8 6 L 9 7.5 L 10 6.5", strokeWidth: 1.1 }),
  ),
  andes: e(
    Fragment,
    null,
    e("path", { d: "M1.5 13 L 5 10 L 7 11 L 9 2 L 11 10 L 13 9 L 15 13" }),
    e("path", { d: "M8 5 L 9 4 L 10 5.5", strokeWidth: 1.1 }),
  ),
  atlas: e("path", {
    d: "M1 13 L 2.5 9 L 4 11 L 5.5 7 L 7 10 L 8.5 6 L 10 10 L 11.5 8 L 13 11 L 15 9",
  }),
  sierra: e("path", {
    d: "M1 13 Q 3 11 4 9 Q 6 6 8 10 Q 10 11 12 7 Q 14 9 15 13",
  }),
};

export const LANDMARK_NAMES = Object.keys(LANDMARK_GLYPHS);

export const LANDMARK_FALLBACK = [
  "fjord",
  "kunlun",
  "iguazu",
  "tasman",
  "norfolk",
  "saharan",
];

/** Pick a landmark name not currently in `used`. Falls back to the
 *  fallback pool if all primary landmarks are taken. */
export function pickLandmark(used: Set<string>): string {
  const free = LANDMARK_NAMES.filter((n) => !used.has(n));
  const pool = free.length > 0 ? free : LANDMARK_FALLBACK;
  return pool[Math.floor(Math.random() * pool.length)];
}
