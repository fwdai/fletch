// The palette's rows, built from the store, and how they group. Pure: the
// dialog asks this once per keystroke and renders what comes back. The
// commands live in `commands.ts`, the things to jump to in `entities.ts`,
// and what they share in `context.ts`; this is the page and section layer.

import {
  editorsFor,
  openEditor,
  preferredEditor,
  rememberEditor,
} from "@/components/TitleBar/OpenInEditor/editors";
import { ACCENTS } from "@/data/providers";
import { activeSurface, surfaceAgent } from "@/store/surface";
import type { AppState } from "@/store/types";
import { agentItems, pageItems, shortcutItems } from "./commands";
import { type PaletteContext, SEC, startDraft } from "./context";
import { entityItems, environmentRows, projectRows, settingsRows } from "./entities";
import { bestMatch } from "./match";
import type { PageId, PaletteItem, Row, Section } from "./types";

export { type PaletteContext, SEC } from "./context";

/** How many rows a section shows while searching; the query narrows the rest. */
const SECTION_CAP: Record<string, number> = { [SEC.agents]: 8 };
const DEFAULT_CAP = 6;
const JUMP_CAP = 6;
const SUGGESTED_CAP = 5;
const RECENT_CAP = 5;

/** Every row the root page can show, before any query is applied. */
export function rootItems(ctx: PaletteContext): PaletteItem[] {
  return [...shortcutItems(ctx), ...agentItems(ctx), ...pageItems(ctx), ...entityItems(ctx)];
}

/** The rows of one page. */
export function pageRows(ctx: PaletteContext, page: PageId): PaletteItem[] {
  const { s } = ctx;
  switch (page) {
    case "root":
      return rootItems(ctx);
    case "new-agent":
      return projectRows(s, (repo) => startDraft(s, repo, "agent"));
    case "workflow":
      return projectRows(s, (repo) => startDraft(s, repo, "workflow"));
    case "environment":
      return environmentRows(s);
    case "settings":
      return settingsRows(ctx, "");
    case "editor": {
      const agent = surfaceAgent(s);
      if (!agent) return [];
      const editors = editorsFor(agent, ctx.editors);
      const preferred = preferredEditor(editors);
      return editors.map((e) => ({
        id: `editor:${e.id}`,
        label: e.label,
        meta: e.id === preferred?.id ? "Preferred" : undefined,
        leading: { kind: "icon", name: e.kind === "terminal" ? "terminal" : "code" },
        section: SEC.editors,
        run: () => {
          rememberEditor(e.id);
          openEditor(agent, e.id).catch((err) => s.setLastError(String(err)));
        },
      }));
    }
    case "accent":
      return ACCENTS.map((a) => ({
        id: `accent:${a.id}`,
        label: a.label,
        meta: a.id === s.accent ? "Current" : undefined,
        leading: { kind: "swatch", color: a.color },
        section: SEC.accent,
        run: () => s.setAccent(a.id),
      }));
  }
}

// ── Grouping ──────────────────────────────────────────────────────────────────

const plain = (items: PaletteItem[]): Row[] => items.map((item) => ({ item, indices: [] }));

/** What the root page shows before anything is typed: a few actions that fit
 *  the surface in front, what was run lately, and the agents worth a look.
 *  The surface decides the first group — stopping is only worth offering
 *  while something runs — and the other two sections fill in behind it. */
export function emptySections(
  s: AppState,
  items: PaletteItem[],
  recents: readonly string[],
): Section[] {
  const byId = new Map(items.map((it) => [it.id, it]));
  const suggested = suggestedFor(s)
    .map((id) => byId.get(id))
    .filter((it): it is PaletteItem => it !== undefined)
    .slice(0, SUGGESTED_CAP);
  const taken = new Set(suggested.map((it) => it.id));
  const recent = recents
    .map((id) => byId.get(id))
    .filter((it): it is PaletteItem => it !== undefined && !taken.has(it.id))
    .slice(0, RECENT_CAP);
  const jump = items
    .filter((it) => it.section === SEC.agents && it.id !== `agent:${s.selectedAgentId}`)
    .slice(0, JUMP_CAP);
  return [
    { label: SEC.suggested, rows: plain(suggested) },
    { label: SEC.recent, rows: plain(recent) },
    { label: SEC.jump, rows: plain(jump) },
  ].filter((sec) => sec.rows.length > 0);
}

/** The ids worth surfacing first on each surface, best first. Missing ids
 *  (an action that doesn't apply) are skipped, so the list reads as what can
 *  be done, not what might. */
function suggestedFor(s: AppState): string[] {
  const hasArchived = (s.workspace?.agents ?? []).some((a) => a.archive);
  switch (activeSurface(s).kind) {
    case "agent":
      return [
        "stopAgent",
        "archiveAgent",
        "commit-pr",
        "view-pr",
        "openInEditor",
        "panelGit",
        "newAgent",
      ];
    case "draft":
      return ["focusComposer", "new-agent-in", "workflow-in", "addProject", "home"];
    case "run":
      return ["home", "newAgent", "togglePanel", "history"];
    case "project":
      return ["projectSettings", "newAgent", "workflow-in", "home"];
    case "home":
      return ["newAgent", "addProject", "workflow-in", hasArchived ? "history" : "usage", "usage"];
    default:
      return ["home", "newAgent", "shortcuts"];
  }
}

/** The rows that answer `query`, grouped by section, best section first and
 *  best row first within it. While there is a query each section is capped —
 *  a longer query is how the rest is reached; with none (a page just opened)
 *  every row shows in its own order. */
export function searchSections(items: PaletteItem[], query: string): Section[] {
  const searching = query.trim().length > 0;
  const scored = new Map<string, (Row & { score: number })[]>();
  for (const item of items) {
    const m = bestMatch(query, item.label, [item.keywords, item.hint].filter(Boolean).join("  "));
    if (!m) continue;
    const list = scored.get(item.section) ?? [];
    list.push({ item, score: m.score, indices: m.indices });
    scored.set(item.section, list);
  }
  return [...scored.entries()]
    .map(([label, rows]) => {
      if (searching) rows.sort((a, b) => b.score - a.score);
      const cap = searching ? (SECTION_CAP[label] ?? DEFAULT_CAP) : rows.length;
      return {
        label,
        best: rows[0].score,
        rows: rows.slice(0, cap).map(({ item, indices }) => ({ item, indices })),
      };
    })
    .sort((a, b) => b.best - a.best)
    .map(({ label, rows }) => ({ label, rows }));
}
