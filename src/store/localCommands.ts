// Dispatcher for app-handled ("local") slash commands — Claude built-ins that
// don't resolve over stream-json. The composer fires `runLocalCommand(action)`
// instead of sending text to the agent (see Composer's onLocalCommand). Two
// mechanisms:
//
//  - `cli:*`  shell out to the real `claude <subcommand>` and render its output
//             as a chat notice (the app can't produce this data itself).
//  - `app:*`  drive existing store/UI capabilities the app already owns.

import type { ChatItem } from "@/adapters";
import type { AgentUsage } from "@/adapters/usage";
import { api } from "@/api";
import { repoPathFor } from "@/helpers";
import type { LocalCommandsSlice, SliceCreator } from "./types";

/** One-line-per-field summary of a session's token usage, for `/cost`. Claude
 *  doesn't report a dollar cost in-transcript, so that line only appears for
 *  agents that do (costUsd > 0). */
function formatUsage(u: AgentUsage | undefined): string {
  if (!u) return "No usage recorded for this session yet.";
  const n = (v: number) => v.toLocaleString();
  const lines = [
    `Input ${n(u.inputTokens)} · Output ${n(u.outputTokens)}`,
    `Cache read ${n(u.cacheReadTokens)} · write ${n(u.cacheWriteTokens)}`,
    `Context ${n(u.contextTokens)}${u.contextWindow ? ` / ${n(u.contextWindow)}` : ""}`,
  ];
  if (u.costUsd > 0) lines.push(`Cost $${u.costUsd.toFixed(4)}`);
  return lines.join("\n");
}

export const createLocalCommandsSlice: SliceCreator<LocalCommandsSlice> = (set, get) => {
  // Push a transient notice into a session's log. These are store-inserted (not
  // persisted to session records), so they read as ephemeral command output and
  // disappear on transcript reload — fine for /doctor output or a /cost readout.
  const append = (agentId: string, entry: ChatItem) =>
    set((s) => ({
      managedLogs: {
        ...s.managedLogs,
        [agentId]: [...(s.managedLogs[agentId] ?? []), entry],
      },
    }));

  // A dim, ambient status line (the user didn't ask to read it — it just says
  // "something is happening"). Right for the transient "Running…" hint.
  const status = (agentId: string, text: string) =>
    append(agentId, { kind: "notice", subtype: "info", text });

  // Prominent, readable output the user explicitly asked for by invoking a
  // command. `label` heads the block with the command name.
  const output = (agentId: string, label: string, text: string, isError = false) =>
    append(agentId, {
      kind: "notice",
      subtype: "command_output",
      label,
      text,
      ...(isError ? { is_error: true } : {}),
    });

  const runClaude = async (agentId: string, args: string[], label: string) => {
    // Picking a local command inserts no composer text, so without this the UI
    // shows nothing for the few seconds the subprocess runs.
    status(agentId, `Running ${label}…`);
    try {
      const out = await api.runClaudeCommand(agentId, args);
      const text = [out.stdout, out.stderr]
        .map((s) => s.trim())
        .filter(Boolean)
        .join("\n\n");
      output(agentId, label, text || `${label} produced no output.`, !out.success);
    } catch (err) {
      output(agentId, label, `${label} failed: ${String(err)}`, true);
    }
  };

  return {
    runLocalCommand: async (action, agentId) => {
      switch (action) {
        case "app:config":
          get().openSettingsScreen();
          return;
        case "app:resume":
          get().toggleHistory(true);
          return;
        case "app:clear": {
          // Claude's /clear resets context; the app equivalent is a fresh
          // session in the same repo (a new draft).
          if (!agentId) return;
          const repoPath = repoPathFor(get(), agentId);
          if (repoPath) await get().createDraft(repoPath);
          return;
        }
        case "app:cost":
          if (agentId) output(agentId, "/cost", formatUsage(get().usage[agentId]));
          return;
        case "cli:doctor":
          if (agentId) await runClaude(agentId, ["doctor"], "/doctor");
          return;
        case "cli:mcp":
          if (agentId) await runClaude(agentId, ["mcp", "list"], "/mcp");
          return;
      }
    },
  };
};
