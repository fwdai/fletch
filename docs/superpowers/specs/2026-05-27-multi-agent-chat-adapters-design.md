# Multi-agent chat adapter layer — design

**Date:** 2026-05-27
**Status:** Approved, ready for implementation planning
**Scope:** Frontend refactor introducing a per-agent view-adapter layer for the custom (structured) chat view, plus the minimum backend change required to thread provider identity through.

## Motivation

The custom chat view currently parses Claude Code's `stream-json` output and Claude session JSONL transcripts with hand-rolled logic in `src/store.ts`. Two related problems:

1. **Claude Code internal wrappers leak into the UI.** Slash-command artifacts (`<command-name>`, `<command-message>`, `<command-args>`, `<local-command-stdout>`, `<local-command-caveat>`) and `<system-reminder>` blocks are rendered verbatim as user-message bubbles. They aren't user-authored content; they're injected by the Claude Code CLI before passing prompts to the model.
2. **The architecture cannot extend to other agents.** The provider list in `data/providers.ts` declares claude, codex, cursor, gemini, opencode, and pi — but the store's event handler, message taxonomy, and transcript replay are all hard-coded to Claude's stream-json shape. There is no abstraction for "given a raw event from agent X, produce a normalized chat item." Adding any second agent today requires editing the core store.

This refactor introduces that abstraction, ports Claude's existing logic into it, and adds a Codex skeleton adapter to validate that the interface generalizes.

## Architectural split: Rust transport / TS view

Per-agent logic divides cleanly along two axes:

- **Transport (Rust).** Process spawn config, stdin envelope for sending user messages, on-disk transcript location, turn-end detection driving backend state. These touch the OS or backend state machines and must live in the host process.
- **View (TypeScript).** Parsing raw events into normalized chat items, filtering noise, choosing how each item renders. These are pure functions over data the backend already forwards to the frontend.

Today Rust already forwards every raw event to the frontend verbatim under `agent:event`. The TS side has no adapter layer yet — it pattern-matches Claude's shape inline. This design adds the TS view layer; Rust changes are limited to a single new field (`provider`) that lets the frontend look up which adapter to use.

When new agent backends ship in Rust later (Codex, Gemini), they will need their own transport implementations: spawn args, stdin envelope, transcript discovery, and `Activity` impls. That work is **out of scope here**; only the `provider` field is added now.

## Scope

**In scope:**

- New `src/adapters/` directory containing a `ChatAdapter` interface, normalized `ChatItem` taxonomy, declarative `DisplayPolicy` type, and shared reducer helpers.
- A real `claudeAdapter` that fully replaces the inline parsing currently in `store.ts`.
- A `codexAdapter` skeleton: real signature, registered in the adapter map, body uses small handcrafted fixtures based on public docs. Marked with `TODO(codex-real-impl)`.
- A `provider: String` field on the Rust `AgentRecord` (default `"claude"` via serde for existing records).
- Deletion of replaced logic from `src/store.ts` (~250 lines).

**Out of scope:**

- Codex/Gemini/Cursor/Pi Rust backends (transport, spawn, transcript discovery).
- Extracting an `AgentTransport` trait in Rust. That happens when the second backend lands and we have two real implementations to abstract over.
- UI for user-toggling display policies. The policy type is shaped so a future settings panel can write overrides into the same lookup; that panel is not built here.
- PTY/native view code paths. The adapter layer is custom-view-only.
- Any changes to spawn arguments, stdin envelope, or session JSONL location for Claude.

## Architecture

### File layout

```
src/adapters/
  index.ts              ADAPTERS registry, getAdapter(provider) helper
  types.ts              ChatAdapter interface, ChatItem union, DisplayPolicy, RawEvent
  policy.ts             applyPolicy(items, policy) → items
  shared/
    json.ts             isRecord, asRecord, asBlockList (generic JSON helpers)
    reducer-helpers.ts  extendLastAssistant, finalizeLastAssistant, upsertToolCall, dedupAgainstLast
  claude/
    index.ts            exports claudeAdapter
    reduce.ts           pure reducer: (prevItems, rawEvent) → ChatItem[]
    normalize.ts        transcript line → synthetic RawEvent[]
    sanitize.ts         strips Claude Code wrapper tags from user text
    policy.ts           DisplayPolicy defaults
    fixtures/
      live-events.jsonl
      transcript.jsonl
      expected.json
    reduce.test.ts
    sanitize.test.ts
  codex/
    index.ts
    reduce.ts
    normalize.ts
    policy.ts
    fixtures/
      sample.jsonl
      expected.json
    reduce.test.ts
```

Files inside each adapter folder collapse during implementation if any turn out trivially small. The split above reflects responsibilities, not a contract.

### Core types

```ts
export type ChatItem =
  | { kind: "user_message"; text: string }
  | { kind: "agent_message"; text: string; streaming?: boolean }
  | { kind: "tool_call"; id: string; name: string; input: unknown; streaming?: boolean }
  | { kind: "tool_result"; tool_use_id: string; content: unknown; is_error?: boolean }
  | {
      kind: "notice";
      subtype:
        | "turn_end"
        | "error"
        | "info"
        | "reasoning"
        | "slash_command"
        | "hook_output";
      text: string;
      is_error?: boolean;
    };

export type RawEvent = Record<string, unknown> & { type?: string };

export type DisplayMode = "show" | "hide";
export type DisplayPolicy = Record<string, DisplayMode>;
// Keys: either `${kind}` or `${kind}:${subtype}`. Subtype key wins.

export interface ChatAdapter {
  readonly id: string;
  reduce(prevItems: ChatItem[], rawEvent: RawEvent): ChatItem[];
  normalizeTranscript(transcriptLines: unknown[]): RawEvent[];
  readonly policy: DisplayPolicy;
}
```

### Adapter registry

`src/adapters/index.ts`:

```ts
import { claudeAdapter } from "./claude";
import { codexAdapter } from "./codex";
import type { ChatAdapter } from "./types";

export const ADAPTERS: Record<string, ChatAdapter> = {
  claude: claudeAdapter,
  codex: codexAdapter,
};

export function getAdapter(provider: string): ChatAdapter {
  const adapter = ADAPTERS[provider];
  if (!adapter) {
    console.warn(`[adapters] unknown provider "${provider}", falling back to claude`);
    return claudeAdapter;
  }
  return adapter;
}
```

### Store integration

`store.ts` replaces `handleManagedEvent`, `transcriptEventsToItems`, and the streaming/dedup helpers with a single `applyEvent`:

```ts
function applyEvent(state: AppState, agentId: string, rawEvent: RawEvent): Partial<AppState> {
  const record = state.workspace?.agents.find((a) => a.id === agentId);
  const adapter = getAdapter(record?.provider ?? "claude");
  const prev = state.managedLogs[agentId] ?? [];

  let next: ChatItem[];
  try {
    next = adapter.reduce(prev, rawEvent);
  } catch (err) {
    console.error("[adapters] reduce threw", { provider: adapter.id, type: rawEvent.type, err });
    return {};
  }
  if (next === prev) return {};

  return { managedLogs: { ...state.managedLogs, [agentId]: next } };
}
```

Transcript replay uses the same path:

```ts
async function loadHistoryTranscript(agentId: string) {
  const lines = await api.readSessionTranscript(agentId);
  const adapter = getAdapter(/* ...provider lookup... */);
  const events = adapter.normalizeTranscript(lines);
  let items: ChatItem[] = [];
  for (const ev of events) {
    items = adapter.reduce(items, ev);
  }
  set({ managedLogs: { ...get().managedLogs, [agentId]: items } });
}
```

`applyPolicy` runs at the render boundary (selector or component), not inside the reducer — keeping policy filtering as a derived view means a future "show hidden items" toggle is one re-derivation away.

### Component layer

`src/components/Workspace/messages/MessageItem.tsx` dispatches on `ChatItem.kind`. The `system` and `result` cases are removed; a new `notice` case switches on `subtype` to pick the visual treatment:

| Subtype | Visual |
|---|---|
| `slash_command` | Small muted bubble, prefixed with the command name |
| `error` | Red-bordered notice (uses existing `--danger` color) |
| `turn_end` | Hidden by default policy; if shown, muted notice |
| `hook_output` | Reasoning-style grey notice |
| `info` | Hidden by default; if shown, muted notice |
| `reasoning` | Hidden by default; if shown, collapsed reasoning block (renderer can be filled in when codex actually emits these) |

Existing CSS classes (`.m-user`, `.m-agent`, `.m-reasoning`) are reused.

## Claude adapter

### Event → ChatItem mapping

| Raw event | Reducer behavior |
|---|---|
| `stream_event` / `content_block_start` (`text`) | Append `agent_message` with `streaming: true`, text seeded from `content_block.text` |
| `stream_event` / `delta` (`text_delta`) | Extend last streaming `agent_message.text` |
| `stream_event` / `content_block_start` (`tool_use`) | Upsert `tool_call` keyed on `id`, `streaming: true` |
| `stream_event` / `delta` (`input_json_delta`) | Append partial JSON string to last `tool_call.input` |
| `assistant` (finalized) | Finalize streaming `agent_message`; append any new text/tool_use blocks not already present in last items |
| `user` (text content) | Sanitize → emit `user_message` if remainder non-empty; emit `slash_command` / `hook_output` notices for stripped wrappers |
| `user` (tool_result blocks) | Append `tool_result` items |
| `result` (success) | If turn had no assistant text and `ev.result` is non-empty, append `agent_message`. Always append `notice { subtype: "turn_end" }` (hidden by policy). |
| `result` (error) | Append `notice { subtype: "error", is_error: true }` |
| unknown event type | Return `prevItems` unchanged |

This is a behavior-preserving port of the current `handleManagedEvent` logic, with one delta: the sanitizer pass on user-message text and the resulting structured notices.

### Sanitizer

`src/adapters/claude/sanitize.ts` exports:

```ts
interface SanitizeResult {
  text: string;
  notices: NoticeItem[];
}
export function sanitizeUserText(raw: string): SanitizeResult;
```

Handled wrapper tags (closed set, expanded only when a new one is encountered):

- `<command-name>NAME</command-name>` plus siblings `<command-message>`, `<command-args>`, `<local-command-stdout>`, `<local-command-caveat>`. Together represent one slash-command invocation. Emits one `notice { subtype: "slash_command", text: "/NAME" }`; strips all related tags.
- `<system-reminder>BODY</system-reminder>`. Hook output or injected system context. Emits `notice { subtype: "hook_output", text: BODY }`; strips the tag.
- Any other unknown `<…>` wrapper is **not** stripped. We don't speculatively remove unfamiliar tags.

If after stripping the visible text is empty, no `user_message` is appended — only the notices (most hidden by default policy).

### Default policy

```ts
export const claudePolicy: DisplayPolicy = {
  "notice:turn_end":      "hide",
  "notice:hook_output":   "hide",
  "notice:info":          "hide",
  "notice:reasoning":     "hide",
  "notice:slash_command": "show",
  "notice:error":         "show",
  // user_message, agent_message, tool_call, tool_result default to "show"
};
```

### Transcript normalization

Claude's JSONL stores finalized `assistant` / `user` / `result` messages (no streaming deltas). `normalizeTranscript` re-emits each recognized line as-is — the same reducer that handles live finalized events handles them correctly. Dedup behavior matches today's `transcriptEventsToItems`.

## Codex adapter (skeleton)

Real `ChatAdapter` signature, registered in `ADAPTERS`. Body:

- `reduce()` pattern-matches a small handcrafted fixture (one or two event shapes from public docs — likely `message` / `function_call` / `function_call_output`) and emits the obvious `ChatItem`s.
- `normalizeTranscript()` mirrors `reduce()` against a small JSONL fixture.
- `policy` initially copies `claudePolicy`; will diverge when real Codex output is observed.
- `fixtures/` contains a handful of sample events and the expected `ChatItem[]`.
- Top-of-file `TODO(codex-real-impl)` comment block citing what's missing and pointing at the work to do when Codex's Rust transport ships.

The skeleton's purpose is to force the interface to be instantiated twice, validating that it generalizes. It is not claimed to be correct against real Codex output.

## Rust changes

Single change: `provider: String` on `AgentRecord` (in `src-tauri/src/workspace.rs`).

```rust
pub struct AgentRecord {
    pub id: String,
    pub name: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    pub repos: Vec<TrackedRepo>,
    // ...rest unchanged
}

fn default_provider() -> String {
    "claude".to_string()
}
```

Migration is implicit: existing workspace JSON without the field deserializes with `provider: "claude"`; saved-back records have it explicitly. No separate migration script.

`spawn_agent` accepts a provider parameter (default `"claude"`) and stamps it on the new record. The TS `api.spawnAgent` call gains a provider arg with the same default. Only `"claude"` flows through the system in this PR, but the field is plumbed end-to-end so frontend `getAdapter(record.provider)` is meaningful.

**Not in this PR:**

- No `AgentTransport` trait extraction. `Activity`, `ManagedSession`, `find_session_jsonl` stay as-is.
- No PTY/native code-path changes.

## Removed from `store.ts`

- `handleManagedEvent` (`store.ts:460`)
- `transcriptEventsToItems` (`store.ts:401`)
- `replayTranscriptEvents` (`store.ts:427`)
- `appendUserIfMissing` (`store.ts:297`)
- `appendItem`, `upsertToolUse`, `updateToolInputDelta`, `updateLastAssistantStreaming`, `finalizeStreamingAssistant`, `mergePatches`, `findLastIndex`
- `contentText`, `transcriptTextContent` → moved into `src/adapters/claude/`
- `isRecord`, `asRecord`, `asBlockList` → moved into `src/adapters/shared/json.ts`
- `ManagedItem` type → replaced by `ChatItem` everywhere referenced (`store.ts`, `MessageItem.tsx`, `ToolUseItem.tsx`, `ToolResultItem.tsx`)

Approximate net deletion in `store.ts`: ~250 lines.

## Error handling

- **Unknown provider** in `getAdapter`: log warning, fall back to `claudeAdapter`. Stale workspace IDs don't crash the chat.
- **Adapter throws inside `reduce`**: store-level try/catch drops the event, logs `{ provider, type, err }`. One malformed event doesn't poison the chat log.
- **Unknown raw event shape**: adapter returns `prevItems` unchanged. Silent — Claude adds new event types regularly and the UI shouldn't break on the next CLI update.
- **Sanitizer matches a wrapper but inner parse fails**: notice is emitted with the raw inner text. Ugly notice > crash.
- **Transcript JSONL missing/malformed**: `normalizeTranscript` returns `[]` for unparseable lines, matching today's `read_session_transcript` behavior.

## Testing strategy

- **Reducer tests are fixture-driven.** For each adapter:
  - `fixtures/live-events.jsonl` — sample raw events.
  - `fixtures/transcript.jsonl` — sample on-disk transcript.
  - `fixtures/expected.json` — expected `ChatItem[]`.
  - Test: `events.reduce(adapter.reduce, []) === expected`.
  - Test: `adapter.normalizeTranscript(lines).reduce(adapter.reduce, [])` produces an equivalent list (modulo streaming-state differences).
- **Sanitizer tests separately.** Unit tests for each wrapper pattern: edge cases include nested tags, malformed tags, multiple wrappers per message, wrapper-only message (text comes back empty).
- **Policy tests.** A few `applyPolicy(items, claudePolicy)` assertions confirming hidden items are dropped, shown items kept.
- **No new integration tests.** Adapter is the unit; the existing store test surface is preserved.
- **Manual smoke before merge.** Run the app, send a message, run a slash command (e.g. `/clear`), restore an archived agent. Verify: wrapper-tag noise is gone, slash command renders as a clean notice, archived chat replay looks identical to live.

## Performance notes

- The reducer is O(n) per event (it builds a new array of length n on streaming deltas). `n` is chat-scale, not data-scale; structural sharing means React only re-renders the changed item. If it ever becomes measurable, swap to an immutable-array library; the adapter contract doesn't change.
- `applyPolicy` runs at the render boundary, not inside the reducer. With memoization on the items array reference, the filter only re-runs when items actually change.

## Open questions / deferred decisions

- **`"collapse"` display mode.** Cut from this design (no item is ever collapsed today). Re-add when a real `reasoning` block needs to render as collapsible.
- **`AgentTransport` trait in Rust.** Deferred until a second backend ships. Doing it speculatively would invent abstractions whose right shape we'll only know once two real transports exist.
- **Shared content-text helpers (`contentText`, `transcriptTextContent`).** Stay claude-specific for now since they encode Anthropic content-block shape. Lift to `shared/` when Codex's real implementation reveals overlap.
- **UI for user-toggling display policies.** The policy type is shaped so a future settings panel can write per-key overrides into the same lookup. Panel itself is a separate project.
