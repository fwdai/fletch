import { useEffect, useMemo, useRef, useState } from "react";
import type { DirListing, PrSummary, TrackerIssue } from "@/api";
import { Icon } from "@/components/Icon";
import { Chip } from "@/components/ui/Chip";
import { IconButton } from "@/components/ui/IconButton";
import { composeIssueBrief } from "@/components/Workspace/MissionControl/inbox";
import { lookupModel, lookupModelInList } from "@/data/modelCatalog";
import {
  PROVIDER_DETAIL,
  type ThinkingLevel,
  thinkingLevelsFromModel,
} from "@/data/providerDetail";
import { DEFAULT_PROVIDER_ID, isDockerSupported, providerLabel } from "@/data/providers";
import type { LocalCommandAction } from "@/data/slashCommands";
import type { AgentUsage } from "@/store";
import { useAppStore } from "@/store";
import { ComposerFrame } from "./ComposerFrame";
import { IssuePicker } from "./IssuePicker";
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
  /** Lists open tracker issues (GitHub, Linear) for the footer issue picker;
   *  picking one appends its brief to the prompt so the agent works that
   *  exact issue. Omit to hide the picker. */
  listIssues?: () => Promise<TrackerIssue[]>;
  /** Fired after an issue pick (in addition to the brief insert), so parents
   *  with a draft can tag it (`issueRef`) for the PR closing trailer. */
  onPickIssue?: (issue: TrackerIssue) => void;
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
   *  session (EmptyWorkspace). The effort picker stays interactive; for a
   *  provider whose effort is a spawn flag (`restartToApply`, e.g. claude),
   *  changing it restarts the session to re-apply the flag (see
   *  `onChangeEffort`). */
  existingSession?: boolean;
  /** For existing sessions: the session's current effort, used to seed the
   *  picker so it reflects the persisted value on load. */
  initialThinking?: string;
  /** For existing sessions: persist a mid-session effort change. Called with
   *  the raw effort value when the user cycles the effort chip. For claude this
   *  restarts the session (resuming it) to re-apply `--effort`; per-turn agents
   *  also carry the value on the next message via `onSend`'s `thinking`. Omit
   *  for new-session composers, where effort is chosen at spawn. */
  onChangeEffort?: (value: string) => void;
  /** For existing sessions: persist a mid-session model change. Called with the
   *  new model id (or undefined for the provider default) when the user picks a
   *  model. For claude this restarts the session to re-apply `--model`; per-turn
   *  agents pick it up on their next turn. Omit for new-session composers, where
   *  the model is chosen at spawn. */
  onChangeModel?: (model: string | undefined) => void;
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

/** The effective thinking level for a provider/model. Every candidate — the
 *  caller's `preferred` value (e.g. a custom agent's saved effort), a stored
 *  per-provider preference, the model default — is used only when the current
 *  model actually supports it, so switching to a model that lacks that level
 *  (e.g. a stale "low", or a custom agent's effort a new model doesn't offer)
 *  falls through rather than sending an unsupported value. Order: preferred →
 *  stored → model default → provider default → highest available. */
function resolveThinking(
  providerId: string,
  levels: ThinkingLevel[],
  modelDefault?: string,
  preferred?: string,
): string | undefined {
  const supports = (v: string | undefined) => !!v && levels.some((l) => l.value === v);
  if (supports(preferred)) return preferred;
  const stored = localStorage.getItem(`thinkingBudget.${providerId}`);
  if (supports(stored ?? undefined)) return stored ?? undefined;
  const d = PROVIDER_DETAIL[providerId as keyof typeof PROVIDER_DETAIL];
  if (supports(modelDefault)) return modelDefault;
  if (supports(d?.defaultLevel)) return d?.defaultLevel;
  return levels.at(-1)?.value;
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
  listIssues,
  onPickIssue,
  seed,
  onSeedConsumed,
  draftKey,
  existingSession = false,
  initialThinking,
  onChangeEffort,
  onChangeModel,
  activeModel,
  usage,
}: Props) {
  const features = useAppStore((s) => s.features);
  const modelCatalog = useAppStore((s) => s.modelCatalog);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const customAgents = useAppStore((s) => s.customAgents);
  const sandboxEngine = useAppStore((s) => s.sandboxEngine);

  // Hide the thinking-effort picker for a model the catalog knows can't reason.
  // When the model is unknown (a new session before the first turn, or one the
  // catalog doesn't list) we keep the picker — better to show a no-op control
  // than to wrongly hide a real one.
  const [provider, setProvider] = useState(defaultProvider);
  const [model, setModel] = useState<string | undefined>(defaultModel);
  const [customAgentId, setCustomAgentId] = useState<string | undefined>(defaultCustomAgentId);
  // Resolve the model within the SELECTED provider's list first: a model id
  // shared across agents keeps only the first-discovered entry in the global
  // `byId` view, which may belong to another provider and omit this provider's
  // per-model metadata (e.g. codex's reasoning levels). Fall back to `byId` for
  // ids the provider list doesn't carry (unknown/new models).
  const activeModelId = existingSession ? (activeModel ?? model) : model;
  const activeMeta =
    lookupModelInList(modelsByAgent[provider], activeModelId) ??
    lookupModel(modelCatalog, activeModelId);
  const modelSupportsThinking = activeMeta ? activeMeta.reasoning : true;

  const detail = PROVIDER_DETAIL[provider as keyof typeof PROVIDER_DETAIL];
  // Prefer the levels the model itself reports (per-model, e.g. codex exposing
  // low→ultra for a given model), falling back to the provider's static list for
  // CLIs that don't report a per-model set. `modelReasoning` is the model's own
  // default level, used to seed the picker. Both inputs are stable references
  // (catalog entry / module const), so the memo — and the effect that depends on
  // it — recompute only on genuine model/provider changes, not every render.
  const modelReasoning = activeMeta?.defaultReasoning;
  const modelReasoningLevels = activeMeta?.reasoningLevels;
  const providerLevels = detail?.thinkingLevels;
  const thinkingLevels = useMemo<ThinkingLevel[]>(() => {
    const modelLevels = thinkingLevelsFromModel(modelReasoningLevels);
    return modelLevels.length > 0 ? modelLevels : (providerLevels ?? []);
  }, [modelReasoningLevels, providerLevels]);

  // A new-agent draft can still hold a docker-unsupported provider chosen
  // before the sandbox engine was switched to Docker. Block the send here —
  // otherwise the stale selection reaches spawnAgent and fails in the backend.
  // `provider` mirrors a custom agent's base, so this covers custom agents too.
  // Existing sessions already spawned with their engine and keep a locked
  // picker, so they're exempt.
  const dockerBlocked =
    !existingSession && sandboxEngine === "docker" && !isDockerSupported(provider);

  const [thinkingValue, setThinkingValue] = useState<string | undefined>(() =>
    resolveThinking(
      defaultProvider,
      thinkingLevels,
      modelReasoning,
      existingSession ? initialThinking : undefined,
    ),
  );

  // Latest custom agents, read via a ref inside the effect below so that
  // editing an agent elsewhere doesn't re-fire it (which would clobber a
  // manually-adjusted thinking level). Kept current on every render.
  const customAgentsRef = useRef(customAgents);
  customAgentsRef.current = customAgents;

  // The session's persisted effort, read via a ref so a change round-tripping
  // back through the `agent:effort` event doesn't re-fire the effect below and
  // clobber the value the user just set (same reasoning as `customAgentsRef`).
  const initialThinkingRef = useRef(initialThinking);
  initialThinkingRef.current = initialThinking;

  // When switching providers or models, restore the last-used level (validated
  // against the new model's supported levels) — a custom agent's own reasoning
  // budget takes precedence, but only when the current model supports it, so it
  // can't send an unsupported value after a model switch (it's passed as the
  // `preferred` candidate, which resolveThinking validates before every other
  // fallback). `thinkingLevels` is memoized, so this fires on genuine
  // provider/model/custom-agent changes, not when the agent list mutates.
  useEffect(() => {
    const custom = customAgentId
      ? customAgentsRef.current.find((a) => a.id === customAgentId)
      : undefined;
    // Existing sessions seed from their persisted effort; new drafts prefer the
    // selected custom agent's saved budget (both validated by resolveThinking).
    const preferred = existingSession ? initialThinkingRef.current : (custom?.effort ?? undefined);
    setThinkingValue(resolveThinking(provider, thinkingLevels, modelReasoning, preferred));
  }, [provider, customAgentId, thinkingLevels, modelReasoning, existingSession]);

  // Shared input core (textarea + `/`·`@`·`#` autocomplete + attachments +
  // draft/seed). `onEnter` sends via a ref so the callback can reference the
  // `input` and `send` defined just below (they depend on `input` in turn).
  const submitRef = useRef<() => void>(() => {});
  const input = useComposerInput({
    provider,
    projectDir,
    // Skills are invocable only where they can still be attached: at spawn.
    // ChatView composers (existing sessions) keep the plain command menu.
    skillCommands: !existingSession,
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
            modelOnly={existingSession}
            onChange={(nextProvider, nextModel, nextCustomAgentId) => {
              // Effort follows from the selection via the effect above (a custom
              // agent's reasoning budget, else the per-provider default).
              setProvider(nextProvider);
              setModel(nextModel);
              setCustomAgentId(nextCustomAgentId);
              if (existingSession) {
                // Model-only change on an existing session: provider/custom-agent
                // are unchanged; persist the new model (backend restarts claude
                // to re-apply --model, per-turn agents pick it up next turn).
                onChangeModel?.(nextModel);
              } else {
                onChangeSelection?.(nextProvider, nextModel, nextCustomAgentId);
              }
            }}
          />
          {features.thinkingBudget && thinkingLevels.length > 0 && modelSupportsThinking && (
            <Chip
              tip={
                existingSession && detail?.restartToApply
                  ? "Thinking effort — changing restarts the agent (rebuilds cache)"
                  : "Thinking budget"
              }
              onClick={() => {
                const idx = thinkingLevels.findIndex((l) => l.value === thinkingValue);
                const next = thinkingLevels[(idx + 1) % thinkingLevels.length];
                setThinkingValue(next.value);
                localStorage.setItem(`thinkingBudget.${provider}`, next.value);
                // Existing sessions persist the change (and, for claude, trigger
                // the session-preserving restart) via the backend; new-session
                // composers just carry it into spawnAgent through `onSend`.
                onChangeEffort?.(next.value);
              }}
            >
              <Icon name="sparkle" size={11} />
              <span>{thinkingLevels.find((l) => l.value === thinkingValue)?.label ?? ""}</span>
            </Chip>
          )}
          <span style={{ flex: 1 }} />
          {/* Insert actions live on the right, beside send: what runs (agent/
           *  model/effort) reads left, what goes into this message reads right. */}
          <IconButton className="composer-action" tip="Attach files" onClick={input.browse}>
            <Icon name="attach" size={15} />
          </IconButton>
          {listIssues && (
            <IssuePicker
              listIssues={listIssues}
              onPick={(issue) => {
                // The brief lands in the prompt for the user to review/edit;
                // the parent additionally tags its draft so the eventual PR
                // closes the issue.
                input.append(composeIssueBrief(issue));
                onPickIssue?.(issue);
              }}
            />
          )}
          <span className="composer-foot-sep" aria-hidden />
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
