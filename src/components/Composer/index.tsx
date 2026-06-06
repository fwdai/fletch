import { useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "../../store";
import { DEFAULT_PROVIDER_ID } from "../../data/providers";
import { PROVIDER_DETAIL } from "../../data/providerDetail";
import { filterCommands, type SlashCommand } from "../../data/slashCommands";
import { Icon } from "../Icon";
import { Chip } from "../ui/Chip";
import { ModelPicker } from "./ModelPicker";
import { BranchPicker } from "./BranchPicker";
import { SlashMenu } from "./SlashMenu";
import { AttachmentList } from "./AttachmentList";
import { useFileDrop } from "./useFileDrop";

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
  /** True when rendered for an existing agent (ChatView) rather than a new
   *  session (EmptyWorkspace). A provider whose effort is set at spawn
   *  (`effortAtSpawn`, e.g. claude) shows a read-only badge here instead of
   *  an interactive picker, since the value can't change mid-session. */
  existingSession?: boolean;
  /** For existing sessions: the effort value this session was spawned with.
   *  Shown as a read-only chip for effortAtSpawn providers (e.g. claude). */
  initialThinking?: string;
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
  existingSession = false,
  initialThinking,
}: Props) {
  const features = useAppStore((s) => s.features);

  const [text, setText] = useState("");
  const [provider, setProvider] = useState(defaultProvider);
  const [attachments, setAttachments] = useState<string[]>([]);

  const detail = PROVIDER_DETAIL[provider as keyof typeof PROVIDER_DETAIL];
  const thinkingLevels = detail?.thinkingLevels ?? [];
  // Preferred initial level, falling back to the highest.
  const defaultThinking = detail?.defaultLevel ?? thinkingLevels.at(-1)?.value;
  const [thinkingValue, setThinkingValue] = useState<string | undefined>(defaultThinking);

  // Reset to the new provider's default (or highest) level when switching.
  useEffect(() => {
    const d = PROVIDER_DETAIL[provider as keyof typeof PROVIDER_DETAIL];
    setThinkingValue(d?.defaultLevel ?? (d?.thinkingLevels ?? []).at(-1)?.value);
  }, [provider]);
  const [slashDismissed, setSlashDismissed] = useState(false);
  const [slashIndex, setSlashIndex] = useState(0);
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
          setSlashDismissed(false);
          grow(e.target);
        }}
        onKeyDown={(e) => {
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
