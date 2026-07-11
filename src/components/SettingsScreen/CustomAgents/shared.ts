// Shared helpers + constants for the Custom Agents settings pane.

/** Preset hues for the monogram tile (evenly spread around the wheel). */
export const CA_HUES = [265, 150, 25, 215, 320, 95, 175, 50] as const;

/** Two-letter monogram derived from an agent's name (initials), falling back to
 *  a neutral dot when unnamed. Mirrors the prototype's `shortFor`. */
export function shortFor(name: string): string {
  const initials = (name || "")
    .trim()
    .split(/\s+/)
    .map((w) => w[0] ?? "")
    .join("")
    .slice(0, 2)
    .toUpperCase();
  return initials || "·";
}

/** How each base provider receives a custom agent's instructions — surfaced as
 *  a hint in the editor so the UI is honest about the per-adapter delivery
 *  (mirrors `instructions.rs`). */
export const INJECTION_HINT: Record<string, string> = {
  claude: "--append-system-prompt",
  codex: "developer instructions (config)",
  cursor: "prepended to the first message",
  opencode: "prepended to the first message",
  antigravity: "prepended to the first message",
  pi: "--append-system-prompt",
};

/** Which MCP transports each base provider can attach — mirrors the backend
 *  delivery in `agent_profile.rs`: claude takes stdio + http via
 *  `--mcp-config`, codex only stdio via `-c mcp_servers.*` (its `-c` config
 *  has no http transport), and the rest have no MCP surface we can drive.
 *  Skills need no such map: the materialize-and-index mechanism works on every
 *  base. Surfaced in the editor so unattachable servers are disabled up front
 *  instead of being silently ignored at spawn. */
export type McpSupport = "all" | "stdio" | "none";
export const MCP_SUPPORT: Record<string, McpSupport> = {
  claude: "all",
  codex: "stdio",
};

/** Editor hint line for the Tools section, by the base's MCP support level. */
export const MCP_HINT: Record<McpSupport, string> = {
  all: "MCP servers attached when this agent runs",
  stdio: "MCP servers attached when this agent runs — command (stdio) servers only",
  none: "Not supported by this base — the agent will run without attached tools",
};
