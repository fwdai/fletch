import { useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "../../store";
import { DEFAULT_PROVIDER_ID } from "../../data/providers";
import { PROVIDER_DETAIL } from "../../data/providerDetail";
import { prettyModelLabel } from "../../data/modelLabel";
import { filterCommands, type SlashCommand } from "../../data/slashCommands";
import { Icon } from "../Icon";
import { Chip } from "../ui/Chip";
import { ModelPicker } from "./ModelPicker";
import { BranchPicker } from "./BranchPicker";
import { SlashMenu } from "./SlashMenu";
import { MentionMenu } from "./MentionMenu";
import { AttachmentList } from "./AttachmentList";
import { useFileDrop } from "./useFileDrop";
import type { DirListing } from "../../api";
import {
  filterDirEntries,
  filterFiles,
  isFsPath,
  joinTypedDir,
  mentionQueryAt,
  mentionTokenEnd,
  splitFsPath,
} from "./mentions";

interface Props {
  /** Initial provider id — defaults to claude. */
  defaultProvider?: string;
  /** When set, render a branch picker chip showing the current base branch. */
  baseBranch?: string;
  /** Repo path used to fetch available branches for the picker. */
  repoPath?: string;
  onChangeBase?: (branch: string) => void;
  placeholder?: string;
  autoFocus?: boolean;
  disabled?: boolean;
  stopping?: boolean;
  /** Fired on Enter (without Shift) or send-button click. `attachments`
   *  holds absolute paths of staged files; the agent receives them as
   *  separate content blocks, kept out of the typed message body. */
  onSend: (payload: {
    text: string;
    provider: string;
    /** Raw effort value for the selected provider, or undefined when the
     *  provider has no thinking levels (e.g. Cursor). */
    thinking: string | undefined;
    attachments: string[];
  }) => void;
  /** Fired when the composer is showing an active stop control. */
  onStop?: () => void;
  /** Fired when the user picks an app-defined slash command. The
   *  `action` identifier comes from the `SlashCommand` entry. The text
   *  is NOT sent to the agent; the parent decides what to do. */
  onLocalCommand?: (action: string) => void;
  /** Supplies candidate worktree-relative file paths for the "@" mention
   *  autocomplete. Called each time a mention opens, so the list stays fresh
   *  as the agent edits files. Omit it (e.g. new sessions with no worktree
   *  yet) to disable "@" mentions; drag-drop / browse attach still work. */
  mentionSource?: () => Promise<string[]>;
  /** Lists an arbitrary directory so "@" can complete filesystem paths the
   *  user types (e.g. `@~/Downloads/`), attaching files outside the worktree
   *  by absolute path. Omit to restrict "@" to worktree files. */
  listDir?: (path: string) => Promise<DirListing>;
  /** True when rendered for an existing agent (ChatView) rather than a new
   *  session (EmptyWorkspace). A provider whose effort is set at spawn
   *  (`effortAtSpawn`, e.g. claude) shows a read-only badge here instead of
   *  an interactive picker, since the value can't change mid-session. */
  existingSession?: boolean;
  /** For existing sessions: the effort value this session was spawned with.
   *  Shown as a read-only chip for effortAtSpawn providers (e.g. claude). */
  initialThinking?: string;
  /** The model the agent actually used on its most recent turn, read from the
   *  transcript (Claude, pi, Codex, OpenCode report it). Shown as a read-only
   *  chip next to the provider so the user can see the real model in use.
   *  Undefined for Cursor / Antigravity (no model in their transcript) or
   *  before the first agent turn. */
  activeModel?: string;
}

function resolveThinking(providerId: string): string | undefined {
  const d = PROVIDER_DETAIL[providerId as keyof typeof PROVIDER_DETAIL];
  const levels = d?.thinkingLevels ?? [];
  const stored = localStorage.getItem(`thinkingBudget.${providerId}`);
  if (stored && levels.some((l) => l.value === stored)) return stored;
  return d?.defaultLevel ?? levels.at(-1)?.value;
}

export function Composer({
  defaultProvider = DEFAULT_PROVIDER_ID,
  baseBranch,
  repoPath,
  onChangeBase,
  placeholder,
  autoFocus,
  disabled,
  stopping = false,
  onSend,
  onStop,
  onLocalCommand,
  mentionSource,
  listDir,
  existingSession = false,
  initialThinking,
  activeModel,
}: Props) {
  const features = useAppStore((s) => s.features);

  const [text, setText] = useState("");
  const [provider, setProvider] = useState(defaultProvider);
  const [attachments, setAttachments] = useState<string[]>([]);

  const detail = PROVIDER_DETAIL[provider as keyof typeof PROVIDER_DETAIL];
  const thinkingLevels = detail?.thinkingLevels ?? [];

  const [thinkingValue, setThinkingValue] = useState<string | undefined>(
    () => resolveThinking(defaultProvider),
  );

  // When switching providers, restore the last-used level for that provider.
  useEffect(() => {
    setThinkingValue(resolveThinking(provider));
  }, [provider]);
  const [slashDismissed, setSlashDismissed] = useState(false);
  const [slashIndex, setSlashIndex] = useState(0);
  // Caret offset, tracked so the "@" mention can be detected at the cursor
  // rather than only at the start of the text (unlike slash commands).
  const [caret, setCaret] = useState(0);
  const [mentionFiles, setMentionFiles] = useState<string[]>([]);
  const [mentionIndex, setMentionIndex] = useState(0);
  const [mentionDismissed, setMentionDismissed] = useState(false);
  // Cached listing for the directory the user is currently typing a path
  // into. `reqDir` is the typed dir it answers, so a stale in-flight result
  // for a different dir isn't shown.
  const [fsListing, setFsListing] = useState<{
    reqDir: string;
    base: string;
    entries: DirListing["entries"];
  } | null>(null);
  const ta = useRef<HTMLTextAreaElement>(null);

  function addPaths(paths: string[]) {
    setAttachments((cur) => {
      const next = [...cur];
      for (const p of paths) if (!next.includes(p)) next.push(p);
      return next;
    });
  }

  const isDropTarget = useFileDrop(addPaths);

  async function browse() {
    const sel = await open({ multiple: true });
    if (!sel) return;
    addPaths(Array.isArray(sel) ? sel : [sel]);
  }

  const slashQuery =
    !slashDismissed && text.startsWith("/") && !text.includes("\n")
      ? text.slice(1).split(/\s/)[0]
      : null;
  const slashMatches = useMemo(
    () => (slashQuery === null ? [] : filterCommands(provider, slashQuery)),
    [provider, slashQuery],
  );
  const slashOpen = slashMatches.length > 0;

  useEffect(() => {
    setSlashIndex(0);
  }, [slashQuery, provider]);

  // "@" mention: active when the caret sits in an "@token" and a source is
  // wired (and the slash menu isn't already claiming the input).
  const mention =
    (mentionSource || listDir) && !slashOpen && !mentionDismissed
      ? mentionQueryAt(text, caret)
      : null;
  // A "~/…" or "/…" style query completes real filesystem paths via listDir;
  // anything else searches the agent's worktree files.
  const fs =
    mention && listDir && isFsPath(mention.query)
      ? splitFsPath(mention.query)
      : null;

  // What picking a row does: attach a file (worktree-relative or absolute),
  // or drill into a directory by rewriting the typed "@query".
  type MentionAction =
    | { kind: "attach"; path: string }
    | { kind: "navigate"; query: string };

  const { rows, actions } = useMemo<{
    rows: { name: string; detail?: string; isDir: boolean }[];
    actions: MentionAction[];
  }>(() => {
    if (!mention) return { rows: [], actions: [] };
    if (fs) {
      if (!fsListing || fsListing.reqDir !== fs.dir) return { rows: [], actions: [] };
      const base = fsListing.base;
      const matched = filterDirEntries(fsListing.entries, fs.partial);
      return {
        rows: matched.map((e) => ({ name: e.name, isDir: e.is_dir })),
        actions: matched.map((e) =>
          e.is_dir
            ? { kind: "navigate", query: joinTypedDir(fs.dir, e.name) }
            : {
                kind: "attach",
                path: base.endsWith("/") ? base + e.name : `${base}/${e.name}`,
              },
        ),
      };
    }
    if (!mentionSource) return { rows: [], actions: [] };
    const matched = filterFiles(mentionFiles, mention.query);
    return {
      rows: matched.map((p) => {
        const i = p.lastIndexOf("/");
        return {
          name: i === -1 ? p : p.slice(i + 1),
          detail: i === -1 ? undefined : p.slice(0, i + 1),
          isDir: false,
        };
      }),
      actions: matched.map((p) => ({ kind: "attach", path: p })),
    };
    // `mention`/`fs` objects recreate each render; depend on their fields.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mention?.query, fs?.dir, fs?.partial, fsListing, mentionFiles, mentionSource]);

  const mentionOpen = rows.length > 0;

  useEffect(() => {
    setMentionIndex(0);
  }, [mention?.query]);

  // Worktree-file mode: refetch the list each time the mention opens (held in
  // a ref so an inline `mentionSource` prop doesn't refire the effect).
  const worktreeActive = mention !== null && !fs && !!mentionSource;
  const mentionSrcRef = useRef(mentionSource);
  mentionSrcRef.current = mentionSource;
  useEffect(() => {
    if (!worktreeActive || !mentionSrcRef.current) return;
    let alive = true;
    mentionSrcRef
      .current()
      .then((files) => {
        if (alive) setMentionFiles(files);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [worktreeActive]);

  // Filesystem mode: re-list only when the typed directory changes, not on
  // every keystroke within the same directory.
  const fsDir = fs?.dir ?? null;
  const listDirRef = useRef(listDir);
  listDirRef.current = listDir;
  useEffect(() => {
    if (fsDir === null || !listDirRef.current) return;
    let alive = true;
    listDirRef
      .current(fsDir)
      .then((res) => {
        if (alive) setFsListing({ reqDir: fsDir, base: res.base, entries: res.entries });
      })
      .catch(() => {
        if (alive) setFsListing(null);
      });
    return () => {
      alive = false;
    };
  }, [fsDir]);

  useEffect(() => {
    if (autoFocus) ta.current?.focus();
  }, [autoFocus]);

  function grow(el: HTMLTextAreaElement) {
    el.style.height = "auto";
    el.style.height = Math.min(el.scrollHeight, 240) + "px";
  }

  function send() {
    const trimmed = text.trim();
    if (stopping) {
      onStop?.();
      return;
    }
    if ((!trimmed && attachments.length === 0) || disabled) return;
    onSend({ text: trimmed, provider, thinking: thinkingValue, attachments });
    setText("");
    setAttachments([]);
    if (ta.current) ta.current.style.height = "auto";
  }

  function stop() {
    if (!stopping) return;
    onStop?.();
  }

  function pickSlash(cmd: SlashCommand) {
    if (cmd.kind === "local") {
      setText("");
      setSlashDismissed(true);
      if (ta.current) ta.current.style.height = "auto";
      onLocalCommand?.(cmd.action);
      return;
    }
    const next = `/${cmd.name} `;
    setText(next);
    setSlashDismissed(true);
    requestAnimationFrame(() => {
      const el = ta.current;
      if (!el) return;
      el.focus();
      el.setSelectionRange(next.length, next.length);
      grow(el);
    });
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

  function pickMention(i: number) {
    const action = actions[i];
    if (!action || !mention) return;
    // Replace the entire "@token", not just up to the caret — the user may
    // have moved the cursor back into the middle of it before picking.
    const end = mentionTokenEnd(text, caret);
    if (action.kind === "attach") {
      addPaths([action.path]);
      // The file lives in the attachment chips, so it never pollutes prose.
      const next = text.slice(0, mention.start) + text.slice(end);
      setText(next);
      setCaret(mention.start);
      placeCaret(mention.start);
    } else {
      // Drill into the directory: rewrite the "@token" to the chosen path so
      // the next keystroke (or selection) continues from inside it.
      const inserted = `@${action.query}`;
      const next = text.slice(0, mention.start) + inserted + text.slice(end);
      const pos = mention.start + inserted.length;
      setText(next);
      setCaret(pos);
      placeCaret(pos);
    }
  }

  const sendDisabled = stopping
    ? !onStop
    : disabled || (!text.trim() && attachments.length === 0);

  return (
    <div className={`composer${isDropTarget ? " is-drop-target" : ""}`}>
      {isDropTarget && (
        <div className="composer-drop-overlay">
          <Icon name="upload" size={20} />
          <span>Drop files to attach</span>
        </div>
      )}
      {slashOpen && (
        <SlashMenu
          commands={slashMatches}
          highlight={slashIndex}
          onPick={pickSlash}
          onHighlight={setSlashIndex}
        />
      )}
      {mentionOpen && (
        <MentionMenu
          items={rows}
          highlight={mentionIndex}
          onPick={pickMention}
          onHighlight={setMentionIndex}
        />
      )}
      {attachments.length > 0 && (
        <AttachmentList
          paths={attachments}
          onRemove={(p) => setAttachments((cur) => cur.filter((x) => x !== p))}
        />
      )}
      <textarea
        ref={ta}
        className="composer-input"
        placeholder={placeholder || "Message agent · /commands · @ to attach · # for PRs"}
        value={text}
        rows={1}
        disabled={disabled}
        onChange={(e) => {
          setText(e.target.value);
          setCaret(e.target.selectionStart ?? e.target.value.length);
          setSlashDismissed(false);
          setMentionDismissed(false);
          grow(e.target);
        }}
        onSelect={(e) => setCaret(e.currentTarget.selectionStart ?? 0)}
        onKeyDown={(e) => {
          if (mentionOpen) {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setMentionIndex((i) => (i + 1) % rows.length);
              return;
            }
            if (e.key === "ArrowUp") {
              e.preventDefault();
              setMentionIndex((i) => (i - 1 + rows.length) % rows.length);
              return;
            }
            if (e.key === "Enter" || e.key === "Tab") {
              e.preventDefault();
              pickMention(mentionIndex);
              return;
            }
            if (e.key === "Escape") {
              e.preventDefault();
              setMentionDismissed(true);
              return;
            }
          }
          if (slashOpen) {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setSlashIndex((i) => (i + 1) % slashMatches.length);
              return;
            }
            if (e.key === "ArrowUp") {
              e.preventDefault();
              setSlashIndex(
                (i) => (i - 1 + slashMatches.length) % slashMatches.length,
              );
              return;
            }
            if (e.key === "Enter" || e.key === "Tab") {
              e.preventDefault();
              const cmd = slashMatches[slashIndex];
              if (cmd) pickSlash(cmd);
              return;
            }
            if (e.key === "Escape") {
              e.preventDefault();
              setSlashDismissed(true);
              return;
            }
          }
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            send();
          }
        }}
      />
      <div className="composer-foot">
        <ModelPicker value={provider} onChange={setProvider} />
        {existingSession && activeModel && (
          <Chip tip={`Model in use · ${activeModel}`}>
            <span style={{ color: "var(--fg-2)" }}>
              {prettyModelLabel(activeModel)}
            </span>
          </Chip>
        )}
        {features.thinkingBudget && thinkingLevels.length > 0 && (
          existingSession && detail?.effortAtSpawn ? (
            initialThinking && (
              <Chip tip="Thinking effort — fixed at spawn" disabled>
                <Icon name="sparkle" size={11} />
                <span>
                  {thinkingLevels.find((l) => l.value === initialThinking)?.label ?? initialThinking}
                </span>
              </Chip>
            )
          ) : (
            <Chip
              tip="Thinking budget"
              onClick={() => {
                const idx = thinkingLevels.findIndex((l) => l.value === thinkingValue);
                const next = thinkingLevels[(idx + 1) % thinkingLevels.length];
                setThinkingValue(next.value);
                localStorage.setItem(`thinkingBudget.${provider}`, next.value);
              }}
            >
              <Icon name="sparkle" size={11} />
              <span>{thinkingLevels.find((l) => l.value === thinkingValue)?.label ?? ""}</span>
            </Chip>
          )
        )}
        {features.autoEdit && (
          <Chip tip="Auto-approve writes">
            <Icon name="check" size={11} />
            <span>Auto-edit</span>
          </Chip>
        )}
        {baseBranch && repoPath && onChangeBase && (
          <BranchPicker
            repoPath={repoPath}
            value={baseBranch}
            onChange={onChangeBase}
          />
        )}
        {baseBranch && (!repoPath || !onChangeBase) && (
          <Chip tip="Base branch">
            <Icon name="branch" size={11} />
            <span style={{ color: "var(--fg-2)" }}>from</span>
            <span style={{ fontFamily: "var(--font-mono)" }}>{baseBranch}</span>
          </Chip>
        )}
        <Chip tip="Attach" onClick={browse}>
          <Icon name="attach" size={11} />
        </Chip>
        <span style={{ flex: 1 }} />
        <button
          type="button"
          className={`send ${stopping ? "is-stop" : ""}`}
          disabled={sendDisabled}
          onClick={stopping ? stop : send}
          aria-label={stopping ? "Stop" : "Send"}
        >
          <Icon name={stopping ? "stop" : "arrowUp"} size={13} />
        </button>
      </div>
    </div>
  );
}
