import { useEffect, useRef, useState } from "react";
import type { DirListing, PrSummary } from "@/api";
import { Icon } from "@/components/Icon";
import { Chip } from "@/components/ui/Chip";
import { lookupModel } from "@/data/modelCatalog";
import { PROVIDER_DETAIL } from "@/data/providerDetail";
import { DEFAULT_PROVIDER_ID, isDockerSupported, providerLabel } from "@/data/providers";
import type { LocalCommandAction } from "@/data/slashCommands";
import type { AgentUsage } from "@/store";
import { useAppStore } from "@/store";
import { ComposerFrame } from "./ComposerFrame";
import { ModelPicker } from "./ModelPicker";
import { UsageMeter } from "./UsageMeter";
import { useComposerInput } from "./useComposerInput";

interface Props {
  /** Initial provider id — defaults to claude. */
  defaultProvider?: string;
  /** Initial model id for new-agent drafts. Undefined means provider default. */
  defaultModel?: string;
  /** Initial custom-agent id for new-agent drafts, if one was selected. */
  defaultCustomAgentId?: string;
  placeholder?: string;
  autoFocus?: boolean;
  disabled?: boolean;
  stopping?: boolean;
  /** Minimum visible lines for the input box (default 1). The new-agent page
   *  uses 2 to give first prompts more room; it stays a floor — the box still
   *  grows with content and never shrinks below this. */
  minRows?: number;
  /** Fired on Enter (without Shift) or send-button click. `attachments`
   *  holds absolute paths of staged files; the agent receives them as
   *  separate content blocks, kept out of the typed message body. */
  onSend: (payload: {
    text: string;
    provider: string;
    model: string | undefined;
    /** Raw effort value for the selected provider, or undefined when the
     *  provider has no thinking levels (e.g. Cursor). */
    thinking: string | undefined;
    /** The selected custom agent's id, or undefined for a built-in spawn. */
    customAgentId: string | undefined;
    attachments: string[];
  }) => void;
  /** Fired when the composer is showing an active stop control. */
  onStop?: () => void;
  /** Fired when the user picks an app-defined slash command. The
   *  `action` identifier comes from the `SlashCommand` entry. The text
   *  is NOT sent to the agent; the parent decides what to do. */
  onLocalCommand?: (action: LocalCommandAction) => void;
  /** The agent's project root, used to discover project-level slash commands
   *  (`<projectDir>/.claude/commands`) for the `/` autocomplete. Omit before a
   *  project is chosen; user-level commands are then still offered. */
  projectDir?: string;
  /** Fired when a new-agent draft changes its provider/model/custom-agent
   *  selection. `customAgentId` is set when a custom agent is picked. */
  onChangeSelection?: (provider: string, model?: string, customAgentId?: string) => void;
  /** Supplies candidate checkout-relative file paths for the "@" mention
   *  autocomplete. Called each time a mention opens, so the list stays fresh
   *  as the agent edits files. Omit it (e.g. new sessions with no checkout
   *  yet) to disable "@" mentions; drag-drop / browse attach still work. */
  mentionSource?: () => Promise<string[]>;
  /** Lists an arbitrary directory so "@" can complete filesystem paths the
   *  user types (e.g. `@~/Downloads/`), attaching files outside the checkout
   *  by absolute path. Omit to restrict "@" to checkout files. */
  listDir?: (path: string) => Promise<DirListing>;
  /** Lists the repo's open PRs for the "#" mention autocomplete, which
   *  inserts a `#<number>` reference. Omit to disable "#" mentions. */
  listPrs?: () => Promise<PrSummary[]>;
  /** Text to inject into the input from elsewhere (e.g. the Git panel's
   *  "→ chat" review-comment action). Appended to whatever is already typed,
   *  then `onSeedConsumed` fires so the parent can clear it. */
  seed?: string;
  onSeedConsumed?: () => void;
  /** Persists unsent text across view switches (which remount this component).
   *  Use the agent id for existing chats, the draft id for the new-agent
   *  composer. The initial value is restored from the store on mount and kept
   *  in sync on every edit; omit it to disable draft persistence. */
  draftKey?: string;
  /** True when rendered for an existing agent (ChatView) rather than a new
   *  session (EmptyWorkspace). A provider whose effort is set at spawn
   *  (`effortAtSpawn`, e.g. claude) shows a read-only badge here instead of
   *  an interactive picker, since the value can't change mid-session. */
  existingSession?: boolean;
  /** For existing sessions: the effort value this session was spawned with.
   *  Shown as a read-only chip for effortAtSpawn providers (e.g. claude). */
  initialThinking?: string;
  /** The model the agent actually used on its most recent turn, read from the
   *  transcript (Claude, pi, Codex, OpenCode report it). Used as a fallback
   *  when the transcript has not yielded a model yet, so the picker can still
   *  reflect the real spawn-time choice. Undefined for Cursor / Antigravity
   *  (no model in their transcript) or before the first agent turn. */
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
  defaultModel,
  defaultCustomAgentId,
  placeholder,
  autoFocus,
  disabled,
  stopping = false,
  minRows = 1,
  onSend,
  onStop,
  onLocalCommand,
  projectDir,
  onChangeSelection,
  mentionSource,
  listDir,
  listPrs,
  seed,
  onSeedConsumed,
  draftKey,
  existingSession = false,
  initialThinking,
  activeModel,
  usage,
}: Props) {
  const features = useAppStore((s) => s.features);
  const modelCatalog = useAppStore((s) => s.modelCatalog);
  const customAgents = useAppStore((s) => s.customAgents);
  const sandboxEngine = useAppStore((s) => s.sandboxEngine);

  // Hide the thinking-effort picker for a model the catalog knows can't reason.
  // When the model is unknown (a new session before the first turn, or one the
  // catalog doesn't list) we keep the picker — better to show a no-op control
  // than to wrongly hide a real one.
  const [provider, setProvider] = useState(defaultProvider);
  const [model, setModel] = useState<string | undefined>(defaultModel);
  const [customAgentId, setCustomAgentId] = useState<string | undefined>(defaultCustomAgentId);
  const activeMeta = lookupModel(modelCatalog, existingSession ? (activeModel ?? model) : model);
  const modelSupportsThinking = activeMeta ? activeMeta.reasoning : true;

  const detail = PROVIDER_DETAIL[provider as keyof typeof PROVIDER_DETAIL];
  const thinkingLevels = detail?.thinkingLevels ?? [];

  // A new-agent draft can still hold a docker-unsupported provider chosen
  // before the sandbox engine was switched to Docker. Block the send here —
  // otherwise the stale selection reaches spawnAgent and fails in the backend.
  // `provider` mirrors a custom agent's base, so this covers custom agents too.
  // Existing sessions already spawned with their engine and keep a locked
  // picker, so they're exempt.
  const dockerBlocked =
    !existingSession && sandboxEngine === "docker" && !isDockerSupported(provider);

  const [thinkingValue, setThinkingValue] = useState<string | undefined>(() =>
    resolveThinking(defaultProvider),
  );

  // Latest custom agents, read via a ref inside the effect below so that
  // editing an agent elsewhere doesn't re-fire it (which would clobber a
  // manually-adjusted thinking level). Kept current on every render.
  const customAgentsRef = useRef(customAgents);
  customAgentsRef.current = customAgents;

  // When switching providers, restore the last-used level for that provider —
  // unless a custom agent is selected with its own reasoning budget, which
  // takes precedence (runs after the provider change a custom pick triggers,
  // so it wins). Fires only on genuine provider/custom-agent *selection*
  // changes, not when the agent list mutates.
  useEffect(() => {
    const custom = customAgentId
      ? customAgentsRef.current.find((a) => a.id === customAgentId)
      : undefined;
    setThinkingValue(custom?.effort || resolveThinking(provider));
  }, [provider, customAgentId]);

  // Shared input core (textarea + `/`·`@`·`#` autocomplete + attachments +
  // draft/seed). `onEnter` sends via a ref so the callback can reference the
  // `input` and `send` defined just below (they depend on `input` in turn).
  const submitRef = useRef<() => void>(() => {});
  const input = useComposerInput({
    provider,
    projectDir,
    onLocalCommand,
    mentionSource,
    listDir,
    listPrs,
    draftKey,
    autoFocus,
    seed,
    onSeedConsumed,
    onEnter: () => submitRef.current(),
  });

  const hasContent = input.text.trim().length > 0 || input.attachments.length > 0;
  // Busy + empty → Stop; busy + typed (or idle) → Send. So a mid-turn
  // follow-up sends with Enter, and an empty composer still stops the turn.
  const showStop = stopping && !hasContent;
  const sendDisabled = showStop ? !onStop : disabled || !hasContent || dockerBlocked;

  function send() {
    // While the agent works, an empty composer's primary action is Stop; once
    // the user types, it becomes Send so the message can be queued/injected
    // mid-turn (the agent keeps running — see store.sendUserMessage).
    if (showStop) {
      onStop?.();
      return;
    }
    const trimmed = input.text.trim();
    if ((!trimmed && input.attachments.length === 0) || disabled || dockerBlocked) return;
    onSend({
      text: trimmed,
      provider,
      model,
      thinking: thinkingValue,
      customAgentId,
      attachments: input.attachments,
    });
    input.clear();
  }
  submitRef.current = send;

  return (
    <ComposerFrame
      input={input}
      placeholder={placeholder || "Message agent · /commands · @ to attach · # for PRs"}
      disabled={disabled}
      minRows={minRows}
      foot={
        <>
          <ModelPicker
            provider={provider}
            model={model}
            customAgentId={customAgentId}
            locked={existingSession}
            onChange={(nextProvider, nextModel, nextCustomAgentId) => {
              // Effort follows from the selection via the effect above (a custom
              // agent's reasoning budget, else the per-provider default).
              setProvider(nextProvider);
              setModel(nextModel);
              setCustomAgentId(nextCustomAgentId);
              onChangeSelection?.(nextProvider, nextModel, nextCustomAgentId);
            }}
          />
          {features.thinkingBudget &&
            thinkingLevels.length > 0 &&
            modelSupportsThinking &&
            (existingSession && detail?.effortAtSpawn ? (
              initialThinking && (
                <Chip tip="Thinking effort — fixed at spawn" disabled>
                  <Icon name="sparkle" size={11} />
                  <span>
                    {thinkingLevels.find((l) => l.value === initialThinking)?.label ??
                      initialThinking}
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
            ))}
          <Chip tip="Attach" onClick={input.browse}>
            <Icon name="attach" size={11} />
          </Chip>
          <span style={{ flex: 1 }} />
          {features.tokenUsage && usage && usage.contextTokens > 0 && <UsageMeter usage={usage} />}
          {/* A disabled <button> swallows hover in the WebView, so the reason
           *  rides a wrapper span that stays hover-capable (same pattern as the
           *  ModelPicker's disabled rows). */}
          <span
            className={dockerBlocked ? "tip" : undefined}
            data-tip={
              dockerBlocked
                ? `${providerLabel(provider)} isn't available in Docker sandboxes yet — switch to Claude to send`
                : undefined
            }
          >
            <button
              type="button"
              className={`send flex-center ${showStop ? "is-stop" : ""}`}
              disabled={sendDisabled}
              onClick={showStop ? () => onStop?.() : send}
              aria-label={showStop ? "Stop" : "Send"}
            >
              <Icon name={showStop ? "stop" : "arrowUp"} size={13} />
            </button>
          </span>
        </>
      }
    />
  );
}
