import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import type { DirListing, PrSummary } from "@/api";
import type { LocalCommandAction } from "@/data/slashCommands";
import { useAppStore } from "@/store";
import { useCommandSource } from "./autocomplete/sources/commands";
import { useFileSource } from "./autocomplete/sources/files";
import { usePrSource } from "./autocomplete/sources/prs";
import { triggerQueryAt } from "./autocomplete/triggers";
import { useAutocomplete } from "./autocomplete/useAutocomplete";
import { useFileDrop } from "./useFileDrop";

/** Max grown height of the input, in px, before it scrolls internally. */
const MAX_HEIGHT = 240;

export interface ComposerInputConfig {
  /** Provider id driving the `/` slash-command source (claude has commands;
   *  other providers use an empty adapter). */
  provider: string;
  /** Project root for discovering project-level slash commands. */
  projectDir?: string;
  /** Offer library skills in the `/` menu (new-agent composers only — an
   *  invoked skill is attached at spawn, which an existing session can't be). */
  skillCommands?: boolean;
  onLocalCommand?: (action: LocalCommandAction) => void;
  /** Candidate file paths for the `@` mention search. Omit to disable `@`. */
  mentionSource?: () => Promise<string[]>;
  /** Lists a directory so `@~/…` completes real filesystem paths. */
  listDir?: (path: string) => Promise<DirListing>;
  /** Lists the repo's open PRs for the `#` mention. Omit to disable `#`. */
  listPrs?: () => Promise<PrSummary[]>;
  /** Persists unsent text across the remounts a view switch causes. */
  draftKey?: string;
  autoFocus?: boolean;
  /** Text injected from elsewhere; appended to the draft, then consumed. */
  seed?: string;
  onSeedConsumed?: () => void;
  /** Fired on Enter without Shift, after the autocomplete menu declines it. */
  onEnter: () => void;
}

/** The reusable composer input core: the textarea's text/caret/attachment
 *  state, the shared `/`·`@`·`#` autocomplete, drag-drop + browse attach,
 *  draft persistence, and seed injection. Both the agent [`Composer`] and the
 *  workflow composer build their own footer + submit on top of this, so the
 *  input behaves identically everywhere. */
export function useComposerInput(cfg: ComposerInputConfig) {
  const { draftKey, autoFocus, seed, onSeedConsumed, onEnter } = cfg;
  const setComposerDraft = useAppStore((s) => s.setComposerDraft);

  // Read the restored draft once at mount via getState (not a subscription) so
  // persisting on each keystroke doesn't re-render; a view switch remounts us
  // and re-runs this initializer.
  const [text, setText] = useState(() =>
    draftKey ? (useAppStore.getState().composerDrafts[draftKey] ?? "") : "",
  );
  // Caret offset, tracked so triggers can be detected at the cursor.
  const [caret, setCaret] = useState(0);
  const [attachments, setAttachments] = useState<string[]>([]);
  const ta = useRef<HTMLTextAreaElement>(null);

  function grow(el: HTMLTextAreaElement) {
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, MAX_HEIGHT)}px`;
  }

  function placeCaret(pos: number) {
    requestAnimationFrame(() => {
      const el = ta.current;
      if (!el) return;
      el.focus();
      el.setSelectionRange(pos, pos);
      grow(el);
    });
  }

  function addPaths(paths: string[]) {
    setAttachments((cur) => {
      const next = [...cur];
      for (const p of paths) if (!next.includes(p)) next.push(p);
      return next;
    });
  }

  function removePath(path: string) {
    setAttachments((cur) => cur.filter((x) => x !== path));
  }

  const isDropTarget = useFileDrop(addPaths);

  async function browse() {
    const sel = await open({ multiple: true });
    if (!sel) return;
    addPaths(Array.isArray(sel) ? sel : [sel]);
  }

  // Autocompletions share one menu + keyboard mechanics (useAutocomplete);
  // each source owns its data and what picking a row does. Triggers are
  // mutually exclusive at a given caret, so only one menu is ever open.
  const fileSource = useFileSource({
    query: triggerQueryAt(text, caret, "@")?.query ?? null,
    mentionSource: cfg.mentionSource,
    listDir: cfg.listDir,
    addPaths,
  });
  const prSource = usePrSource({
    query: triggerQueryAt(text, caret, "#")?.query ?? null,
    listPrs: cfg.listPrs,
  });
  const commandSource = useCommandSource({
    query: triggerQueryAt(text, caret, "/", true)?.query ?? null,
    provider: cfg.provider,
    projectDir: cfg.projectDir,
    includeSkills: cfg.skillCommands,
    onLocalCommand: cfg.onLocalCommand,
  });
  const autocomplete = useAutocomplete({
    sources: [commandSource, fileSource, prSource],
    text,
    caret,
    setText,
    setCaret,
    focusAt: placeCaret,
  });

  useEffect(() => {
    if (autoFocus) ta.current?.focus();
  }, [autoFocus]);

  // On mount (including the remount a view switch causes) a restored multi-line
  // draft renders at single-row height until grow() runs on an edit; resize once
  // so it fits its content.
  // biome-ignore lint/correctness/useExhaustiveDependencies: run once on mount
  useEffect(() => {
    if (ta.current) grow(ta.current);
  }, []);

  // Mirror the draft into the store on every edit so it survives the remount a
  // view switch causes. Clearing `text` on send clears the entry.
  useEffect(() => {
    if (draftKey) setComposerDraft(draftKey, text);
  }, [draftKey, text, setComposerDraft]);

  /** Append text to the draft (blank-line separated), then focus, resize,
   *  and land the caret at the end. Used by the external `seed` prop and by
   *  in-composer inserts (e.g. the issue picker's brief). */
  function append(extra: string) {
    setText((cur) => {
      const next = cur.trim() ? `${cur}\n\n${extra}` : extra;
      requestAnimationFrame(() => {
        const el = ta.current;
        if (!el) return;
        el.focus();
        grow(el);
        const end = next.length;
        el.setSelectionRange(end, end);
        setCaret(end);
      });
      return next;
    });
  }

  // Apply an externally-supplied seed, and notify the parent to clear it.
  // biome-ignore lint/correctness/useExhaustiveDependencies: runs only when a new seed arrives, not on append identity
  useEffect(() => {
    if (!seed) return;
    append(seed);
    onSeedConsumed?.();
  }, [seed, onSeedConsumed]);

  /** Reset after a successful send/launch: clear text + attachments and shrink. */
  function clear() {
    setText("");
    setAttachments([]);
    if (ta.current) ta.current.style.height = "auto";
  }

  return {
    text,
    setText,
    append,
    caret,
    attachments,
    addPaths,
    removePath,
    ta,
    autocomplete,
    isDropTarget,
    browse,
    clear,
    /** Spread onto the `<textarea>` — Enter-to-submit defers to the menu first. */
    textareaHandlers: {
      onChange: (e: React.ChangeEvent<HTMLTextAreaElement>) => {
        setText(e.target.value);
        setCaret(e.target.selectionStart ?? e.target.value.length);
        grow(e.target);
      },
      onSelect: (e: React.SyntheticEvent<HTMLTextAreaElement>) =>
        setCaret(e.currentTarget.selectionStart ?? 0),
      onKeyDown: (e: React.KeyboardEvent) => {
        if (autocomplete.onKeyDown(e)) return;
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          onEnter();
        }
      },
    },
  };
}

export type ComposerInput = ReturnType<typeof useComposerInput>;
