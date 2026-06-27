import { beforeEach, describe, expect, it, vi } from "vitest";

// Mock the generic db layer so we can assert the exact row shape the storage
// module hands to SQLite, without a real Tauri backend.
const dbInsert = vi.fn(async (..._a: unknown[]) => "ok");
const dbUpdate = vi.fn(async (..._a: unknown[]) => 1);
const dbDelete = vi.fn(async (..._a: unknown[]) => 1);
const dbSelect = vi.fn(async (..._a: unknown[]) => [] as unknown[]);

vi.mock("./db", () => ({
  dbInsert: (...a: unknown[]) => dbInsert(...a),
  dbUpdate: (...a: unknown[]) => dbUpdate(...a),
  dbDelete: (...a: unknown[]) => dbDelete(...a),
  dbSelect: (...a: unknown[]) => dbSelect(...a),
}));

import {
  type CustomAgent,
  createCustomAgent,
  deleteCustomAgent,
  listCustomAgents,
  updateCustomAgent,
} from "./customAgents";

const NEW = {
  name: "Reviewer",
  description: "Critical reviewer",
  color: 25,
  base: "codex",
  model: "gpt-5.2-codex",
  effort: "high",
  instructions: "Be terse.",
};

describe("customAgents storage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-24T00:00:00Z"));
  });

  it("stamps id + timestamps on create and inserts the full row", async () => {
    const agent = await createCustomAgent(NEW);
    expect(agent.id).toMatch(/^ca-/);
    expect(agent.created_at).toBe(agent.updated_at);
    expect(agent.created_at).toBe(Date.now());

    const [table, row] = dbInsert.mock.calls[0] as unknown as [string, CustomAgent];
    expect(table).toBe("custom_agents");
    expect(row).toMatchObject({ ...NEW, id: agent.id });
  });

  it("bumps updated_at and never writes the id/created_at columns on update", async () => {
    const current: CustomAgent = {
      ...NEW,
      id: "ca-x",
      created_at: 1000,
      updated_at: 1000,
    };
    vi.setSystemTime(new Date("2026-06-25T00:00:00Z"));
    const next = await updateCustomAgent(current, { name: "Renamed" });

    expect(next.name).toBe("Renamed");
    expect(next.created_at).toBe(1000);
    expect(next.updated_at).toBe(Date.now());

    const [table, where, data] = dbUpdate.mock.calls[0] as unknown as [
      string,
      Record<string, unknown>,
      Record<string, unknown>,
    ];
    expect(table).toBe("custom_agents");
    expect(where).toEqual({ id: "ca-x" });
    // The primary key and creation time must not be in the UPDATE set.
    expect(data).not.toHaveProperty("id");
    expect(data).not.toHaveProperty("created_at");
    expect(data.name).toBe("Renamed");
  });

  it("lists newest-edited first", async () => {
    await listCustomAgents();
    expect(dbSelect).toHaveBeenCalledWith("custom_agents", {
      orderBy: "updated_at",
      orderDirection: "desc",
    });
  });

  it("deletes by id", async () => {
    await deleteCustomAgent("ca-x");
    expect(dbDelete).toHaveBeenCalledWith("custom_agents", { id: "ca-x" });
  });
});
