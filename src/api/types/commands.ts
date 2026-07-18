/** A user- or project-level slash command found on disk by
 *  `discover_slash_commands` (e.g. a `~/.claude/commands/*.md`). Mirrors the
 *  Rust `DiscoveredCommand`; always maps to a `passthrough` command in the
 *  composer. `scope` is "user" or "project" ("project" shadows "user"). */
export interface DiscoveredCommand {
  name: string;
  description: string;
  hint?: string;
  scope: "user" | "project";
}

/** Captured output of a one-shot `claude <args>` invocation run for a local
 *  slash command (e.g. `/doctor`). Mirrors the Rust `ClaudeCommandOutput`. */
export interface ClaudeCommandOutput {
  stdout: string;
  stderr: string;
  success: boolean;
}
