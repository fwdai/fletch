import { invoke } from "../invoke";
import type { ClaudeCommandOutput, DiscoveredCommand } from "../types/commands";

export const commandsApi = {
  discoverSlashCommands: (provider: string, projectDir?: string) =>
    invoke<DiscoveredCommand[]>("discover_slash_commands", {
      provider,
      projectDir: projectDir ?? null,
    }),
  runClaudeCommand: (agentId: string, args: string[]) =>
    invoke<ClaudeCommandOutput>("run_claude_command", { agentId, args }),
};
