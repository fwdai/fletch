// The last placeholder on the Roadmap surface: the product map.
//
// The board left this file when items became rows in `roadmap_items` (loaded by
// `useRoadmap`), and the PM conversation left it when the thread became a real
// agent chat (see Thread/usePmChats.ts). What's left is the map the board's
// second tab draws — a static picture of the codebase's domains, until the PM
// agent can derive it from the repo it reads.

import type { MapDomain } from "./types";

export const PRODUCT_MAP: MapDomain[] = [
  {
    id: "runtime",
    label: "Agent runtime",
    note: "spawner · worktrees · providers",
    files: 42,
    items: 3,
    heat: "hot",
  },
  {
    id: "chrome",
    label: "App chrome",
    note: "title bar · panels · settings",
    files: 61,
    items: 3,
    heat: "warm",
  },
  {
    id: "planning",
    label: "Planning",
    note: "backlog · roadmap · this page",
    files: 18,
    items: 1,
    heat: "warm",
  },
  {
    id: "vcs",
    label: "Git & review",
    note: "diffs · PRs · checks",
    files: 34,
    items: 0,
    heat: "cool",
  },
  {
    id: "platform",
    label: "Workflows",
    note: "steps · loops · runs",
    files: 27,
    items: 0,
    heat: "cool",
  },
];
