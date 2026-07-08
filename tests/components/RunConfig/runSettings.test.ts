import { beforeEach, describe, expect, it, vi } from "vitest";

// Back the projectSettings layer with an in-memory map so the two run-config
// scopes (project `run.<id>`, agent `run.agent.<agentId>.<id>`) can be
// exercised without a real Tauri backend.
let stored: Record<string, string> = {};

vi.mock("@/storage/projectSettings", () => ({
  getProjectSettings: async () => ({ ...stored }),
  setProjectSetting: async (_p: string, key: string, value: string) => {
    stored[key] = value;
  },
  deleteProjectSetting: async (_p: string, key: string) => {
    delete stored[key];
  },
}));

import { loadRunOverrides, persistRunOverrides } from "@/components/RunConfig";

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("run settings scoping", () => {
  beforeEach(() => {
    stored = {};
  });

  it("loads project-scope values without slurping agent-scoped keys", async () => {
    stored = {
      "run.dev": "npm run dev",
      "run.agent.a1.dev": "npm run dev -- --port 4000",
      "other.key": "ignored",
    };
    await expect(loadRunOverrides("p1")).resolves.toEqual({ dev: "npm run dev" });
  });

  it("loads only the requested agent's overrides", async () => {
    stored = {
      "run.dev": "npm run dev",
      "run.agent.a1.dev": "bun dev",
      "run.agent.a1.port": "4000",
      "run.agent.a2.dev": "yarn dev",
    };
    await expect(loadRunOverrides("p1", "a1")).resolves.toEqual({ dev: "bun dev", port: "4000" });
    await expect(loadRunOverrides("p1", "a2")).resolves.toEqual({ dev: "yarn dev" });
  });

  it("persists to the project scope by default and the agent scope when given", async () => {
    persistRunOverrides("p1", [{ id: "dev", value: "bun dev" }], []);
    persistRunOverrides("p1", [{ id: "port", value: "4000" }], [], "a1");
    await flush();
    expect(stored).toEqual({ "run.dev": "bun dev", "run.agent.a1.port": "4000" });
  });

  it("deletes from the matching scope only", async () => {
    stored = { "run.dev": "bun dev", "run.agent.a1.dev": "yarn dev" };
    persistRunOverrides("p1", [], ["dev"], "a1");
    await flush();
    expect(stored).toEqual({ "run.dev": "bun dev" });
    persistRunOverrides("p1", [], ["dev"]);
    await flush();
    expect(stored).toEqual({});
  });
});
