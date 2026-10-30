import { open } from "@tauri-apps/plugin-dialog";
import { stat } from "@tauri-apps/plugin-fs";
import { useEffect, useRef, useState } from "react";
import { api, type DirListing, type FolderGrant, type PrSummary } from "@/api";
import type { LocalCommandAction } from "@/data/slashCommands";
import { useAppStore } from "@/store";
import { activeEnvironment } from "@/store/environments";
import { useCommandSource } from "./autocomplete/sources/commands";
import { useFileSource } from "./autocomplete/sources/files";
import { usePrSource } from "./autocomplete/sources/prs";
import { triggerQueryAt } from "./autocomplete/triggers";
import { useAutocomplete } from "./autocomplete/useAutocomplete";
import { stageAttachmentPath, stageEnvironmentPath } from "./stageAttachment";
import { useFileDrop } from "./useFileDrop";

/** Max grown height of the input, in px, before it scrolls internally. Exported
 *  for the frame, which sizes the box to the dictation ghost as well. */
export const MAX_HEIGHT = 240;

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
  /** Folders can be attached — given to the agent live, at their own paths —
   *  from the `@` listing or a drop. Only where there is an agent to give
   *  them to; elsewhere a dropped folder is refused like any unreadable file. */
  folders?: boolean;
  /** Persists unsent text across the remounts a view switch causes. */
  draftKey?: string;
  autoFocus?: boolean;
  /** Text injected from elsewhere; appended to the draft, then consumed. */
  seed?: string;
  onSeedConsumed?: () => void;
  /** Fired on Enter without Shift, after the autocomplete menu declines it
   *  (and after `onKeyDown`, if that didn't claim the key). */
  onEnter?: () => void;
  /** First look at every key the autocomplete menu declines. Return true to
   *  claim it — the agent composer routes Enter, Escape and ⌘⇧D through here
   *  because what they do depends on dictation and the agent's state. */
  onKeyDown?: (e: React.KeyboardEvent) => boolean;
}

/** Resize a textarea to fit its content, capped at `MAX_HEIGHT`. Exported for
 *  the dictation hook, which writes text the user didn't type. */
export function grow(el: HTMLTextAreaElement) {
  el.style.height = "auto";
  el.style.height = `${Math.min(el.scrollHeight, MAX_HEIGHT)}px`;
}

/** The reusable composer input core: the textarea's text/caret/attachment
 *  state, the shared `/`·`@`·`#` autocomplete, drag-drop + browse attach,
 *  draft persistence, and seed injection. Both the agent [`Composer`] and the
 *  workflow composer build their own footer + submit on top of this, so the
 *  input behaves identically everywhere. */
export function useComposerInput(cfg: ComposerInputConfig) {
  const { draftKey, autoFocus, seed, onSeedConsumed, onEnter, onKeyDown } = cfg;
  const setComposerDraft = useAppStore((s) => s.setComposerDraft);
  const setLastError = useAppStore((s) => s.setLastError);

  // Read the restored draft once at mount via getState (not a subscription) so
  // persisting on each keystroke doesn't re-render; a view switch remounts us
  // and re-runs this initializer.
  const [text, setText] = useState(() =>
    draftKey ? (useAppStore.getState().composerDrafts[draftKey] ?? "") : "",
  );
  // Caret offset, tracked so triggers can be detected at the cursor and
  // dictation can insert there. Starts at the end of the restored draft, which
  // is where the browser puts the caret of a textarea given a value.
  const [caret, setCaret] = useState(text.length);
  const [attachments, setAttachments] = useState<string[]>([]);
  // Folders waiting for the next send, which grants them (read-only unless
  // the user flips one) before the message goes out.
  const [folders, setFolders] = useState<FolderGrant[]>([]);
  const ta = useRef<HTMLTextAreaElement>(null);

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

  function addFolder(path: string) {
    setFolders((cur) =>
      cur.some((f) => f.path === path) ? cur : [...cur, { path, writable: false }],
    );
  }

  function removeFolder(path: string) {
    setFolders((cur) => cur.filter((f) => f.path !== path));
  }

  function toggleFolder(path: string) {
    setFolders((cur) => cur.map((f) => (f.path === path ? { ...f, writable: !f.writable } : f)));
  }

  /** Attach whatever each job stages, and say so once for the ones that
   *  failed — a file too big for the host, one that could not be read. The
   *  rest still attach: losing four attachments because the fifth was a video
   *  is worse than the error. */
  async function stageAll(noun: string, jobs: Promise<string>[]) {
    const results = await Promise.allSettled(jobs);
    addPaths(results.flatMap((r) => (r.status === "fulfilled" ? [r.value] : [])));
    const failed = results.filter((r) => r.status === "rejected");
    if (failed.length) {
      const reason = failed[0].reason;
      setLastError(
        `Couldn't attach ${failed.length} ${noun}${failed.length > 1 ? "s" : ""}: ${
          reason instanceof Error ? reason.message : String(reason)
        }`,
      );
    }
  }

  /** Paths from a drop or the picker. Staged rather than attached as-is, so
   *  the composer only ever holds paths the environment being driven can
   *  read (see `stageAttachmentPath`). A dropped folder becomes a folder chip
   *  instead: it is granted, never copied. */
  const attachPaths = async (paths: string[]) => {
    const dirs = cfg.folders ? await droppedFolders(paths) : [];
    const files = paths.filter((p) => !dirs.includes(p));
    if (dirs.length && activeEnvironment().kind !== "local") {
      setLastError("A folder on this Mac can't be given to an agent running on another machine.");
    } else {
      for (const dir of dirs) addFolder(dir);
    }
    if (files.length) await stageAll("file", files.map(stageAttachmentPath)).catch(() => {});
  };

  const isDropTarget = useFileDrop((paths) => void attachPaths(paths));

  async function browse() {
    const sel = await open({ multiple: true });
    if (!sel) return;
    await attachPaths(Array.isArray(sel) ? sel : [sel]);
  }

  /** Files on the clipboard — a screenshot's image data, or files copied in
   *  Finder — arrive as bytes with no path, so each is written out by the
   *  backend and staged by the returned path. Plain text falls through to the
   *  browser's default paste. Files that fail to read or save are reported in
   *  the global error banner; the rest still attach. */
  async function pasteFiles(files: FileList) {
    // Copied image data has no real name (WebKit labels it "image.png"), so
    // stamp one; a Finder copy keeps its own.
    await stageAll(
      "pasted file",
      Array.from(files).map(async (f, i) => {
        const ext = f.type.split("/")[1] || "bin";
        const generic = !f.name || f.name === "image.png";
        const name = generic ? `pasted-${Date.now()}-${i}.${ext}` : f.name;
        return api.savePastedAttachment(name, new Uint8Array(await f.arrayBuffer()));
      }),
    );
  }

  // Autocompletions share one menu + keyboard mechanics (useAutocomplete);
  // each source owns its data and what picking a row does. Triggers are
  // mutually exclusive at a given caret, so only one menu is ever open.
  const fileSource = useFileSource({
    query: triggerQueryAt(text, caret, "@")?.query ?? null,
    mentionSource: cfg.mentionSource,
    listDir: cfg.listDir,
    addPaths,
    // An `@~/…` pick names a file outside the checkout, staged like a drop so
    // the agent can read it from wherever on this Mac it came from.
    attachFsPaths: (paths) =>
      void stageAll("file", paths.map(stageEnvironmentPath)).catch(() => {}),
    // The listed folder itself: a path on the environment being driven, as
    // the listing is, so it is granted there as it stands.
    attachFsDir: cfg.folders ? addFolder : undefined,
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
    setFolders([]);
    if (ta.current) ta.current.style.height = "auto";
  }

  /** Put back a draft whose send failed after `clear` (a refused folder, a
   *  failed spawn), unless the user has started a new one meanwhile. */
  function restore(draft: { text: string; attachments: string[]; folders: FolderGrant[] }) {
    setText((cur) => (cur ? cur : draft.text));
    setAttachments((cur) => (cur.length ? cur : draft.attachments));
    setFolders((cur) => (cur.length ? cur : draft.folders));
    placeCaret(draft.text.length);
  }

  return {
    text,
    setText,
    append,
    caret,
    /** For a caller that writes text itself (dictation's commit) and has to
     *  tell the input where the caret ended up; `placeCaret` moves the DOM's. */
    setCaret,
    attachments,
    addPaths,
    removePath,
    /** Folder chips for the next send (see `ComposerInputConfig.folders`). */
    folders,
    acceptsFolders: !!cfg.folders,
    removeFolder,
    toggleFolder,
    ta,
    autocomplete,
    isDropTarget,
    browse,
    clear,
    restore,
    /** Spread onto the `<textarea>` — Enter-to-submit defers to the menu first. */
    textareaHandlers: {
      onChange: (e: React.ChangeEvent<HTMLTextAreaElement>) => {
        setText(e.target.value);
        setCaret(e.target.selectionStart ?? e.target.value.length);
        grow(e.target);
      },
      onSelect: (e: React.SyntheticEvent<HTMLTextAreaElement>) =>
        setCaret(e.currentTarget.selectionStart ?? 0),
      onPaste: (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
        const files = e.clipboardData.files;
        if (files.length === 0) return;
        e.preventDefault();
        void pasteFiles(files);
      },
      onKeyDown: (e: React.KeyboardEvent) => {
        if (autocomplete.onKeyDown(e)) return;
        if (onKeyDown?.(e)) return;
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          onEnter?.();
        }
      },
    },
  };
}

export type ComposerInput = ReturnType<typeof useComposerInput>;

/** Which of `paths` (just dropped) are folders. The drop handler granted the
 *  webview's fs scope each dropped path (`src-tauri/src/lib.rs`), so `stat`
 *  may read them; one it can't read is left to the file path, which reports
 *  why. */
async function droppedFolders(paths: string[]): Promise<string[]> {
  const kinds = await Promise.all(
    paths.map((p) =>
      stat(p).then(
        (info) => info.isDirectory,
        () => false,
      ),
    ),
  );
  return paths.filter((_, i) => kinds[i]);
}
