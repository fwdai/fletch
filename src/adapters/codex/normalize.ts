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
// function calls (which cover both shell `exec_command` and MCP calls).
// `response_item` user/assistant messages are skipped — they duplicate the
// event_msg ones and carry injected noise (AGENTS.md, permissions blurb).

import type { RawEvent } from "../types";
import { asRecord } from "../shared/json";

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

export function normalizeTranscript(lines: unknown[]): RawEvent[] {
  // Pre-pass: a tool call's output lands on a later `function_call_output`
  // line, so index outputs by call_id first.
  const outputs = new Map<string, string>();
  for (const raw of lines) {
    const env = asRecord(raw);
    if (env.type !== "response_item") continue;
    const p = asRecord(env.payload);
    if (p.type === "function_call_output") {
      const id = String(p.call_id ?? "");
      if (id) {
        outputs.set(
          id,
          typeof p.output === "string" ? p.output : JSON.stringify(p.output ?? ""),
        );
      }
    }
  }

  const out: RawEvent[] = [];
  for (const raw of lines) {
    const env = asRecord(raw);
    const p = asRecord(env.payload);
    const ptype = typeof p.type === "string" ? p.type : "";

    if (env.type === "event_msg") {
      if (ptype === "user_message") {
        const text = typeof p.message === "string" ? p.message : "";
        if (text) out.push({ type: "user", text });
      } else if (ptype === "agent_message") {
        const text = typeof p.message === "string" ? p.message : "";
        if (text) {
          out.push({
            type: "item.completed",
            item: { id: `msg_${out.length}`, type: "agent_message", text },
          });
        }
      } else if (ptype === "task_complete") {
        out.push({ type: "turn.completed" });
      }
      continue;
    }

    if (env.type === "response_item" && ptype === "function_call") {
      const id = String(p.call_id ?? "");
      if (!id) continue;
      const name = typeof p.name === "string" ? p.name : "";
      const namespace = typeof p.namespace === "string" ? p.namespace : "";
      const args = parseArgs(p.arguments);
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
