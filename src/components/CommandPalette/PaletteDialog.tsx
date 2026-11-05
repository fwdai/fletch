import { open as openExternal } from "@tauri-apps/plugin-shell";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import type { DetectedEditor } from "@/api";
import { Icon } from "@/components/Icon";
import { developerUnlocked } from "@/components/SettingsScreen/nav";
import { detectEditors } from "@/components/TitleBar/OpenInEditor/editors";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore } from "@/store";
import { arrowTarget } from "@/util/arrowNav";
import { effectiveCombos, matchesCombo, SHORTCUT_BY_ID } from "@/util/keymap";
import { useRuns } from "@/workflows/run/useRuns";
import { emptySections, type PaletteContext, pageRows, searchSections } from "./items";
import { PaletteFooter } from "./PaletteFooter";
import { PaletteRow } from "./PaletteRow";
import { loadRecents, noteRecent } from "./recents";
import { PAGES, type PageId, type PaletteItem, type Row } from "./types";

/** Above the overlays (History at 400), below the toasts and tooltips. */
const Z = 450;

/** Whether `e` is the chord that opens the palette — the one key the dialog
 *  lets through to the window, so the same press closes it. */
function isPaletteChord(e: KeyboardEvent): boolean {
  const overrides = useAppStore.getState().shortcutOverrides;
  return effectiveCombos(SHORTCUT_BY_ID.palette, overrides).some((c) => matchesCombo(e, c));
}

/** The open palette. Mounted fresh each time it opens, so the page, query
 *  and cursor always start clean. Owns the keyboard: arrows move the cursor
 *  over every row across sections, Enter runs or drills in, Escape (and a
 *  Backspace on an empty query) backs out of a page or closes. Everything
 *  else is stopped at the dialog so the app's own window listeners (find in
 *  the conversation, the ⌘-digit panels) stay out of it. */
export function PaletteDialog() {
  const togglePalette = useAppStore((s) => s.togglePalette);
  const setLastError = useAppStore((s) => s.setLastError);
  const close = useCallback(() => togglePalette(false), [togglePalette]);

  const [page, setPage] = useState<PageId>("root");
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [editors, setEditors] = useState<DetectedEditor[]>([]);
  const recents = useMemo(() => loadRecents(), []);
  const runs = useRuns();
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let alive = true;
    detectEditors().then((found) => {
      if (alive) setEditors(found);
    });
    return () => {
      alive = false;
    };
  }, []);

  // Focus returns to where it was when the palette is dismissed — not when a
  // row ran, since the row may have put it somewhere on purpose (the composer).
  const ran = useRef(false);
  const returnTo = useRef<Element | null>(document.activeElement);
  useEffect(
    () => () => {
      const el = returnTo.current;
      if (!ran.current && el instanceof HTMLElement && el.isConnected) el.focus();
    },
    [],
  );

  // The rows are rebuilt when anything they read changes. The picked fields
  // are the re-render trigger; the context itself is read whole, since the
  // builders ask `store/surface` what is in front the way the shortcuts do.
  // `trigger` changes identity exactly when a field the builders read does.
  const trigger = useAppStore(
    useShallow((s) => [
      s.workspace,
      s.drafts,
      s.pendingToolUse,
      s.delegations,
      s.environments,
      s.activeEnvironmentId,
      s.features,
      s.theme,
      s.accent,
      s.leftCollapsed,
      s.rightCollapsed,
      s.selectedAgentId,
      s.activeDraftId,
      s.selectedRunId,
      s.settingsScreenOpen,
      s.usageScreenOpen,
      s.projectScreenRepoPath,
      s.historyOpen,
      s.lastRepoPath,
      s.updateCheckStatus,
      s.updateReadyVersion,
      s.admin,
    ]),
  );
  // biome-ignore lint/correctness/useExhaustiveDependencies: trigger is the re-run signal
  const items = useMemo<PaletteItem[]>(() => {
    const s = useAppStore.getState();
    const ctx: PaletteContext = {
      s,
      runs,
      editors,
      developer: developerUnlocked(s.admin),
      now: Date.now(),
      openUrl: (url) => void openExternal(url).catch((e) => setLastError(String(e))),
      copyText: (text) =>
        void navigator.clipboard.writeText(text).catch((e) => setLastError(String(e))),
    };
    return pageRows(ctx, page);
  }, [trigger, page, runs, editors, setLastError]);

  const sections = useMemo(
    () =>
      page === "root" && !query.trim()
        ? emptySections(useAppStore.getState(), items, recents)
        : searchSections(items, query),
    [items, page, query, recents],
  );

  const rows = useMemo<Row[]>(() => sections.flatMap((sec) => sec.rows), [sections]);
  const indexOf = useMemo(() => new Map(rows.map((r, i) => [r.item.id, i])), [rows]);

  // A new query or page starts the cursor at the top; a shorter list keeps
  // it in range.
  // biome-ignore lint/correctness/useExhaustiveDependencies: query and page are the reset triggers
  useEffect(() => setActive(0), [query, page]);
  const cursor = Math.min(active, Math.max(0, rows.length - 1));

  useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>(`[data-index="${cursor}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [cursor]);

  // The mouse moves the cursor only when it actually moves: scrolling a row
  // into view from the keyboard slides the list under a resting pointer, and
  // the mousemove that fires then must not snatch the cursor back.
  const pointer = useRef<{ x: number; y: number } | null>(null);
  const onHover = useCallback((i: number, e: React.MouseEvent) => {
    const last = pointer.current;
    pointer.current = { x: e.clientX, y: e.clientY };
    if (last && last.x === e.clientX && last.y === e.clientY) return;
    setActive(i);
  }, []);

  const back = useCallback(() => {
    setPage("root");
    setQuery("");
  }, []);

  const activate = useCallback(
    (item: PaletteItem) => {
      if (item.disabled) return;
      if (item.page) {
        setPage(item.page);
        setQuery("");
        return;
      }
      ran.current = true;
      if (item.remember) noteRecent(item.id);
      close();
      item.run?.();
    },
    [close],
  );

  function onKeyDown(e: React.KeyboardEvent<HTMLDivElement>) {
    if (isPaletteChord(e.nativeEvent)) return;
    e.stopPropagation();
    switch (e.key) {
      case "ArrowDown":
      case "ArrowUp":
      case "Home":
      case "End": {
        const target = arrowTarget(
          rows.map((_, i) => i),
          cursor,
          e.key,
          { wrap: true },
        );
        if (target === undefined) return;
        e.preventDefault();
        setActive(target);
        return;
      }
      case "Enter": {
        e.preventDefault();
        const row = rows[cursor];
        if (row) activate(row.item);
        return;
      }
      case "Escape":
        e.preventDefault();
        if (page === "root") close();
        else back();
        return;
      case "Backspace":
        if (!query && page !== "root") {
          e.preventDefault();
          back();
        }
        return;
      case "Tab":
        e.preventDefault();
        return;
    }
  }

  const spec = PAGES[page];
  const activeId = rows[cursor] ? `cp-opt-${cursor}` : undefined;

  return (
    <>
      <Scrim onClose={close} zIndex={Z} blur />
      <div
        className="cp"
        style={{ zIndex: Z + 1 }}
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        onKeyDown={onKeyDown}
      >
        <div className="cp-head flex-center">
          {page === "root" ? (
            <Icon name="search" size={16} />
          ) : (
            <button
              type="button"
              className="cp-crumb iflex-center"
              // A click goes back without taking the caret from the input.
              onMouseDown={(e) => e.preventDefault()}
              onClick={back}
            >
              <Icon name="chevL" size={12} />
              {spec.title}
            </button>
          )}
          <input
            className="cp-input"
            autoFocus
            role="combobox"
            aria-expanded="true"
            aria-controls="cp-list"
            aria-activedescendant={activeId}
            aria-autocomplete="list"
            placeholder={spec.placeholder}
            value={query}
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => setQuery(e.target.value)}
          />
          <kbd className="kbd">esc</kbd>
        </div>

        <div
          id="cp-list"
          className="cp-list"
          role="listbox"
          ref={listRef}
          // Keep the caret in the input: a click picks, it never moves focus.
          onMouseDown={(e) => e.preventDefault()}
        >
          {rows.length === 0 ? (
            <div className="cp-empty text-sm">
              <strong className="text-base">No matches</strong>
              {query ? `Nothing answers to “${query}”.` : "Nothing to show here yet."}
            </div>
          ) : (
            sections.map((sec) => (
              <div key={sec.label} className="cp-sect">
                <div className="cp-sect-l text-xs">{sec.label}</div>
                {sec.rows.map((row) => {
                  const i = indexOf.get(row.item.id) ?? 0;
                  return (
                    <PaletteRow
                      key={row.item.id}
                      row={row}
                      index={i}
                      active={i === cursor}
                      onHover={(e) => onHover(i, e)}
                      onActivate={() => activate(row.item)}
                    />
                  );
                })}
              </div>
            ))
          )}
        </div>

        <PaletteFooter page={page} />
      </div>
    </>
  );
}
