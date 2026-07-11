import { dbDelete, dbInsert, dbSelect, dbUpdate } from "./db";

// MCP servers: a shared registry of tool servers custom agents can attach
// (custom_agents.mcp_server_ids). `command`/`env` describe a stdio server,
// `url`/`headers` an http one; env and headers are stored as the user typed
// them (KEY=VALUE / "Name: value" lines) and parsed only when the spawn
// snapshot is built (see snapshotMcpServer). Persisted in `mcp_servers`.

export type McpTransport = "stdio" | "http";

export interface McpServer {
  id: string;
  name: string;
  transport: McpTransport;
  /** Full command line for a stdio server, whitespace-split at spawn
   *  (e.g. "npx -y @modelcontextprotocol/server-github"). */
  command: string;
  /** KEY=VALUE per line, passed as the stdio server's environment. */
  env: string;
  /** Endpoint for an http server. */
  url: string;
  /** "Name: value" per line, sent as http request headers. */
  headers: string;
  created_at: number;
  updated_at: number;
}

/** A new server before it's persisted: everything except the db-managed id and
 *  timestamps. */
export type NewMcpServer = Omit<McpServer, "id" | "created_at" | "updated_at">;

/** The by-value form sent to the backend at spawn and snapshotted onto the
 *  session — mirrors `agent_profile::McpServerSnapshot`. */
export interface McpServerSnapshot {
  name: string;
  transport: McpTransport;
  command: string;
  args: string[];
  env: [string, string][];
  url: string;
  headers: [string, string][];
}

const TABLE = "mcp_servers";

/** Generate a stable, collision-resistant id for a new server. */
function newId(): string {
  return `mcp-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

/** All servers, newest-edited first. */
export async function listMcpServers(): Promise<McpServer[]> {
  return dbSelect<McpServer>(TABLE, {
    orderBy: "updated_at",
    orderDirection: "desc",
  });
}

/** Insert a new server and return the persisted row. */
export async function createMcpServer(server: NewMcpServer): Promise<McpServer> {
  const now = Date.now();
  const row: McpServer = { ...server, id: newId(), created_at: now, updated_at: now };
  await dbInsert(TABLE, row as unknown as Record<string, unknown>);
  return row;
}

/** Patch an existing server (bumping `updated_at`) and return the merged row so
 *  callers can update local state without a re-read. */
export async function updateMcpServer(
  current: McpServer,
  patch: Partial<NewMcpServer>,
): Promise<McpServer> {
  const next: McpServer = { ...current, ...patch, updated_at: Date.now() };
  const { id, created_at, ...writable } = next;
  void id;
  void created_at;
  await dbUpdate(TABLE, { id: current.id }, writable as unknown as Record<string, unknown>);
  return next;
}

export async function deleteMcpServer(id: string): Promise<void> {
  await dbDelete(TABLE, { id });
}

/** Parse `KEY=VALUE` lines into pairs, skipping blanks and lines without `=`.
 *  Values may contain `=` (split on the first one). */
export function parseKeyValueLines(text: string): [string, string][] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && line.includes("="))
    .map((line): [string, string] => {
      const eq = line.indexOf("=");
      return [line.slice(0, eq).trim(), line.slice(eq + 1).trim()];
    })
    .filter(([k]) => k.length > 0);
}

/** Parse `Name: value` header lines into pairs, skipping blanks and lines
 *  without `:`. */
export function parseHeaderLines(text: string): [string, string][] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && line.includes(":"))
    .map((line): [string, string] => {
      const colon = line.indexOf(":");
      return [line.slice(0, colon).trim(), line.slice(colon + 1).trim()];
    })
    .filter(([k]) => k.length > 0);
}

/** Resolve a registry row into the by-value snapshot sent at spawn: the command
 *  line is whitespace-split into command + args (no shell quoting — document
 *  args with spaces as unsupported), env/header lines are parsed into pairs. */
export function snapshotMcpServer(server: McpServer): McpServerSnapshot {
  const tokens = server.command.trim().split(/\s+/).filter(Boolean);
  return {
    name: server.name,
    transport: server.transport,
    command: tokens[0] ?? "",
    args: tokens.slice(1),
    env: parseKeyValueLines(server.env),
    url: server.url.trim(),
    headers: parseHeaderLines(server.headers),
  };
}
