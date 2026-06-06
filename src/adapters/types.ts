// Multi-agent chat adapter contracts. Every per-agent adapter
// produces values from the ChatItem union; the renderer is agnostic
// to which adapter produced any given item.
//
// See docs/superpowers/specs/2026-05-27-multi-agent-chat-adapters-design.md
// for the design rationale.

export type ChatItem =
  | { kind: "user_message"; text: string; attachments?: string[] }
  | {
      kind: "agent_message";
      text: string;
      streaming?: boolean;
      /** The model that produced this turn, when the agent reports it in its
       *  transcript: Claude/pi `message.model` (live + replay); Codex
       *  `turn_context.model` and OpenCode message-blob `modelID` (replay only —
       *  their live streams omit it). Absent for Cursor (model only in the live
       *  `init` event, not the on-disk transcript) and Antigravity (no model in
       *  the transcript) — consumers fall back to the static provider label. */
      model?: string;
    }
  | {
      kind: "tool_call";
      id: string;
      name: string;
      input: unknown;
      streaming?: boolean;
    }
  | {
      kind: "tool_result";
      tool_use_id: string;
      content: unknown;
      is_error?: boolean;
    }
  | {
      kind: "notice";
      subtype: NoticeSubtype;
      text: string;
      is_error?: boolean;
    };

export type NoticeSubtype =
  | "turn_end"
  | "error"
  | "info"
  | "reasoning"
  | "slash_command"
  | "compact_summary"
  | "hook_output";

export type RawEvent = Record<string, unknown> & { type?: string };

export type DisplayMode = "show" | "hide";

// Keys are either `${kind}` or `${kind}:${subtype}`. The more specific
// `${kind}:${subtype}` entry wins when both are present.
export type DisplayPolicy = Record<string, DisplayMode>;

export interface ChatAdapter {
  readonly id: string;
  reduce(prevItems: ChatItem[], rawEvent: RawEvent): ChatItem[];
  normalizeTranscript(transcriptLines: unknown[]): RawEvent[];
  readonly policy: DisplayPolicy;
}
