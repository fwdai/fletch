import { dbInsert, dbSelect, dbDelete, dbCount, dbQuery } from "./db";

export interface MessageRow {
  id: string;
  agent_id: string;
  kind: string;
  content: string;
  metadata_json: string | null;
  sequence: number;
  created_at: number;
}

export interface SearchResult {
  id: string;
  agent_id: string;
  agent_name: string;
  agent_task: string;
  kind: string;
  content: string;
  sequence: number;
  created_at: number;
}

export async function insertMessage(
  data: Omit<MessageRow, "id" | "created_at">,
): Promise<string> {
  return dbInsert("messages", data as Record<string, unknown>);
}

export async function listMessages(
  agentId: string,
  opts?: { limit?: number; offset?: number },
): Promise<MessageRow[]> {
  return dbSelect<MessageRow>("messages", {
    where: { agent_id: agentId },
    orderBy: "sequence",
    orderDirection: "asc",
    ...(opts?.limit && { limit: opts.limit }),
    ...(opts?.offset && { offset: opts.offset }),
  });
}

export async function countMessages(agentId: string): Promise<number> {
  return dbCount("messages", { agent_id: agentId });
}

export async function deleteMessages(agentId: string): Promise<void> {
  await dbDelete("messages", { agent_id: agentId });
}

export async function searchMessages(
  query: string,
  opts?: { agentId?: string; limit?: number },
): Promise<SearchResult[]> {
  const limit = opts?.limit ?? 50;

  if (opts?.agentId) {
    return dbQuery<SearchResult>(
      `SELECT m.id, m.agent_id, a.name AS agent_name, a.task AS agent_task,
              m.kind, m.content, m.sequence, m.created_at
       FROM messages m
       JOIN messages_fts ON messages_fts.rowid = m.rowid
       JOIN agents a ON a.id = m.agent_id
       WHERE messages_fts MATCH ?1 AND m.agent_id = ?2
       ORDER BY rank
       LIMIT ?3`,
      [query, opts.agentId, limit],
    );
  }

  return dbQuery<SearchResult>(
    `SELECT m.id, m.agent_id, a.name AS agent_name, a.task AS agent_task,
            m.kind, m.content, m.sequence, m.created_at
     FROM messages m
     JOIN messages_fts ON messages_fts.rowid = m.rowid
     JOIN agents a ON a.id = m.agent_id
     WHERE messages_fts MATCH ?1
     ORDER BY rank
     LIMIT ?2`,
    [query, limit],
  );
}
