import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "../../store";
import { DEFAULT_PROVIDER_ID } from "../../data/providers";
import { PROVIDER_DETAIL } from "../../data/providerDetail";
import { lookupModel } from "../../data/modelCatalog";
import { prettyModelLabel } from "../../data/modelLabel";
import { Icon } from "../Icon";
import { Chip } from "../ui/Chip";
import { ModelPicker } from "./ModelPicker";
import { BranchPicker } from "./BranchPicker";
import { UsageMeter } from "./UsageMeter";
import type { AgentUsage } from "../../store";
import { AttachmentList } from "./AttachmentList";
import { useFileDrop } from "./useFileDrop";
import type { DirListing, PrSummary } from "../../api";
import { triggerQueryAt } from "./autocomplete/triggers";
import { useAutocomplete } from "./autocomplete/useAutocomplete";
import { AutocompleteMenu } from "./autocomplete/AutocompleteMenu";
import { useFileSource } from "./autocomplete/sources/files";
import { usePrSource } from "./autocomplete/sources/prs";
import { useCommandSource } from "./autocomplete/sources/commands";

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
  /** Lists the repo's open PRs for the "#" mention autocomplete, which
   *  inserts a `#<number>` reference. Omit to disable "#" mentions. */
  listPrs?: () => Promise<PrSummary[]>;
  /** Text to inject into the input from elsewhere (e.g. the Git panel's
   *  "→ chat" review-comment action). Appended to whatever is already typed,
   *  then `onSeedConsumed` fires so the parent can clear it. */
  seed?: string;
  onSeedConsumed?: () => void;
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
  /** Per-agent token usage for the context gauge in the foot. Omit for new
   *  sessions (no agent yet) or agents that report no usage (cursor,
   *  antigravity) — the gauge then hides. */
  usage?: AgentUsage;
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
  listPrs,
  seed,
  onSeedConsumed,
  existingSession = false,
  initialThinking,
  activeModel,
  usage,
}: Props) {
  const features = useAppStore((s) => s.features);
  const modelCatalog = useAppStore((s) => s.modelCatalog);

  // Hide the thinking-effort picker for a model the catalog knows can't reason.
  // When the model is unknown (a new session before the first turn, or one the
  // catalog doesn't list) we keep the picker — better to show a no-op control
  // than to wrongly hide a real one.
  const activeMeta = lookupModel(modelCatalog, activeModel);
  const modelSupportsThinking = activeMeta ? activeMeta.reasoning : true;

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
  // Caret offset, tracked so triggers can be detected at the cursor (not just
  // at the start of the text).
  const [caret, setCaret] = useState(0);
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

  // Autocompletions share one menu + keyboard mechanics (useAutocomplete);
  // each source owns its data and what picking a row does. Triggers are
  // mutually exclusive at a given caret, so only one menu is ever open.
  const fileSource = useFileSource({
    query: triggerQueryAt(text, caret, "@")?.query ?? null,
    mentionSource,
    listDir,
    addPaths,
  });
  const prSource = usePrSource({
    query: triggerQueryAt(text, caret, "#")?.query ?? null,
    listPrs,
  });
  const commandSource = useCommandSource({
    query: triggerQueryAt(text, caret, "/", true)?.query ?? null,
    provider,
    onLocalCommand,
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

  // Apply an externally-supplied seed: append to the current draft (with a
  // blank-line separator), focus, resize, and notify the parent to clear it.
  useEffect(() => {
    if (!seed) return;
    setText((cur) => {
      const next = cur.trim() ? `${cur}\n\n${seed}` : seed;
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
    onSeedConsumed?.();
  }, [seed, onSeedConsumed]);

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

  function placeCaret(pos: number) {
    requestAnimationFrame(() => {
      const el = ta.current;
      if (!el) return;
      el.focus();
      el.setSelectionRange(pos, pos);
      grow(el);
    });
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
      {autocomplete.menu && <AutocompleteMenu {...autocomplete.menu} />}
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
          grow(e.target);
        }}
        onSelect={(e) => setCaret(e.currentTarget.selectionStart ?? 0)}
        onKeyDown={(e) => {
          if (autocomplete.onKeyDown(e)) return;
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
        {features.thinkingBudget && thinkingLevels.length > 0 && modelSupportsThinking && (
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
        {usage && usage.contextTokens > 0 && <UsageMeter usage={usage} />}
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
