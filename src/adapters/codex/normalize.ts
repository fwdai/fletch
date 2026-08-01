// Translates Codex's on-disk rollout file into the live-event shapes the
// reducer understands, so re-attaching to an agent replays its history.
//
// The rollout (`$CODEX_HOME/sessions/.../rollout-*-<id>.jsonl`) is a
// different, dual-channel schema from the live `exec --json` stream:
//   { "type":"session_meta" | "turn_context", … }                  // metadata
//   { "type":"event_msg",     "payload":{ "type":"user_message" | "agent_message" | "task_complete" | … } }
//   { "type":"response_item", "payload":{ "type":"message" | "function_call" | "function_call_output" | "reasoning" | … } }
//
// We take the conversational backbone (user/agent text, turn end) from the
// clean `event_msg` channel, and tool activity from `response_item`
// function/custom-tool calls (which cover shell, MCP, and app tools).
// `response_item` user/assistant messages are skipped — they duplicate the
// event_msg ones and carry injected noise (AGENTS.md, permissions blurb).

import { asRecord } from "@/adapters/shared/json";
import type { RawEvent } from "@/adapters/types";

function parseArgs(v: unknown): unknown {
  if (typeof v === "string") {
    try {
      return JSON.parse(v);
    } catch {
      return v;
    }
  }
  return v ?? {};
}

/** Flatten Codex's persisted output shapes. Legacy function calls store a
 * string; current custom tools store Responses-style input_text blocks. */
function outputText(v: unknown): string {
  if (typeof v === "string") return v;
  if (Array.isArray(v)) {
    return v
      .map((block) => {
        const rec = asRecord(block);
        return typeof rec.text === "string" ? rec.text : "";
      })
      .join("");
  }
  return v == null ? "" : JSON.stringify(v);
}

function reasoningSummary(v: unknown): string {
  if (!Array.isArray(v)) return "";
  return v
    .map((part) => {
      const rec = asRecord(part);
      return typeof rec.text === "string" ? rec.text : "";
    })
    .filter(Boolean)
    .join("\n");
}

export function normalizeTranscript(lines: unknown[]): RawEvent[] {
  // Pre-pass: a tool call's output lands on a later function/custom-tool
  // output line, so index outputs by call_id first.
  const outputs = new Map<string, string>();
  // The rollout encrypts reasoning and often persists no readable summary.
  // Fletch therefore stores the completed live event alongside the rollout;
  // index that text here and place it at the matching response_item below.
  const liveReasoning = new Map<string, string>();
  for (const raw of lines) {
    const env = asRecord(raw);
    if (env.type === "item.completed") {
      const item = asRecord(env.item);
      if (
        item.type === "reasoning" &&
        typeof item.id === "string" &&
        typeof item.text === "string"
      ) {
        liveReasoning.set(item.id, item.text);
      }
      continue;
    }
    if (env.type !== "response_item") continue;
    const p = asRecord(env.payload);
    if (p.type === "function_call_output" || p.type === "custom_tool_call_output") {
      const id = String(p.call_id ?? "");
      if (id) {
        outputs.set(id, outputText(p.output));
      }
    }
  }

  const out: RawEvent[] = [];
  // The model is reported on `turn_context` records (`payload.model`), which
  // precede the turn's events. Track the latest and stamp it onto the agent
  // messages that follow so the UI can show the model in use on replay.
  let currentModel: string | undefined;
  for (const raw of lines) {
    const env = asRecord(raw);
    const p = asRecord(env.payload);
    const ptype = typeof p.type === "string" ? p.type : "";

    if (env.type === "turn_context") {
      const m = p.model;
      if (typeof m === "string") currentModel = m;
      continue;
    }

    if (env.type === "event_msg") {
      if (ptype === "user_message") {
        const text = typeof p.message === "string" ? p.message : "";
        if (text) out.push({ type: "user", text });
      } else if (ptype === "agent_message") {
        const text = typeof p.message === "string" ? p.message : "";
        if (text) {
          out.push({
            type: "item.completed",
            item: { id: `msg_${out.length}`, type: "agent_message", text, model: currentModel },
          });
        }
      } else if (ptype === "task_complete") {
        out.push({ type: "turn.completed" });
      }
      continue;
    }

    if (env.type === "response_item" && ptype === "reasoning") {
      const id = String(p.id ?? "");
      const text = liveReasoning.get(id) ?? reasoningSummary(p.summary);
      if (id && text) {
        out.push({ type: "item.completed", item: { id, type: "reasoning", text } });
      }
      continue;
    }

    if (
      env.type === "response_item" &&
      (ptype === "function_call" || ptype === "custom_tool_call")
    ) {
      const id = String(p.call_id ?? "");
      if (!id) continue;
      const name = typeof p.name === "string" ? p.name : "";
      const namespace = typeof p.namespace === "string" ? p.namespace : "";
      const args = parseArgs(ptype === "custom_tool_call" ? p.input : p.arguments);
      const output = outputs.get(id) ?? "";

      const argRec = asRecord(args);
      if (namespace) {
        // MCP tool call (namespace e.g. "mcp__server_name").
        out.push({
          type: "item.completed",
          item: {
            id,
            type: "mcp_tool_call",
            server: namespace.replace(/^mcp__/, ""),
            tool: name,
            arguments: args,
            result: output,
            status: "completed",
          },
        });
      } else if (name === "exec_command" || typeof argRec.cmd === "string") {
        // Shell / exec command. Parse the real exit code out of codex's
        // wrapped output so a failed command isn't replayed as a success.
        const m = output.match(/exited with code (\d+)/);
        const code = m ? Number(m[1]) : undefined;
        const command = typeof argRec.cmd === "string" ? argRec.cmd : name;
        out.push({
          type: "item.completed",
          item: {
            id,
            type: "command_execution",
            command,
            aggregated_output: output,
            exit_code: code,
            status: code !== undefined && code !== 0 ? "failed" : "completed",
          },
        });
      } else {
        // Other built-in tool (apply_patch, update_plan, …): render as a
        // named tool call preserving its arguments rather than mislabeling
        // it as a shell command and dropping the args.
        out.push({
          type: "item.completed",
          item: {
            id,
            type: "mcp_tool_call",
            server: "",
            tool: name,
            arguments: args,
            result: output,
            status: "completed",
          },
        });
      }
    }
  }
  return out;
}
