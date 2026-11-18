import { describe, expect, it } from "vitest";
import type { ProjectDefinition } from "@/api";
import type { HostProtocol, HostProvider } from "@/remote/types";
import {
  type EnvironmentEntry,
  type EnvironmentId,
  LOCAL_ENVIRONMENT_ID,
} from "@/store/environments";
import { GATES } from "@/store/gates";
import {
  homeName,
  pushedNote,
  type RunTargetFacts,
  runTargetOptions,
  travelReason,
} from "./options";

const THIS_MAC: EnvironmentEntry = {
  id: LOCAL_ENVIRONMENT_ID,
  name: "This Mac",
  kind: "local",
  connection: "connected",
};

/** A protocol that answers everything this client gates on. */
const CURRENT: HostProtocol = {
  version: 9,
  ops: ["ensure_project", "project_definition", "spawn_agent"],
  events: [],
  features: [],
};

const host = (id: string, over: Partial<EnvironmentEntry> = {}): EnvironmentEntry => ({
  id,
  name: id[0].toUpperCase() + id.slice(1),
  kind: "remote",
  connection: "connected",
  protocol: CURRENT,
  ...over,
});

const signedIn: HostProvider = {
  id: "claude",
  label: "Claude Code",
  installed: true,
  version: "2.0.0",
  auth: "signed_in",
  loginCommand: "claude login",
};

const definition = (origins: (string | null)[]): ProjectDefinition => ({
  project_id: "p1",
  name: "Fletch",
  home: { device: "this-mac-key", name: "MacBook" },
  repos: origins.map((origin, i) => ({
    id: `r${i}`,
    dir: `repo${i}`,
    origin,
    label: i === 1 ? "Gateway" : null,
  })),
});

function facts(
  environments: EnvironmentEntry[],
  over: Partial<RunTargetFacts> = {},
): RunTargetFacts {
  return {
    environments: Object.fromEntries(environments.map((e) => [e.id, e])) as Record<
      EnvironmentId,
      EnvironmentEntry
    >,
    draftHost: LOCAL_ENVIRONMENT_ID,
    definition: definition(["https://github.com/acme/fletch.git"]),
    ...over,
  };
}

const option = (f: RunTargetFacts, hostId: string) =>
  runTargetOptions(f).find((o) => o.hostId === hostId);

describe("runTargetOptions", () => {
  it("offers only This Mac with no paired host", () => {
    expect(runTargetOptions(facts([THIS_MAC]))).toEqual([
      {
        id: LOCAL_ENVIRONMENT_ID,
        kind: "local",
        hostId: LOCAL_ENVIRONMENT_ID,
        label: "This Mac",
        available: true,
        reason: null,
      },
    ]);
  });

  it("lists This Mac first, then each paired host by name", () => {
    const f = facts([host("studio"), THIS_MAC, host("mini")]);
    expect(runTargetOptions(f).map((o) => [o.kind, o.label])).toEqual([
      ["local", "This Mac"],
      ["host", "Mini"],
      ["host", "Studio"],
    ]);
    expect(runTargetOptions(f).every((o) => o.available)).toBe(true);
  });

  it("refuses a host that is not connected, in the words its chip uses", () => {
    const f = facts([THIS_MAC, host("studio", { connection: "disconnected", retrying: false })]);
    expect(option(f, "studio")).toMatchObject({ available: false, reason: "offline" });
    const back = facts([THIS_MAC, host("studio", { connection: "error", retrying: true })]);
    expect(option(back, "studio")?.reason).toBe("reconnecting…");
  });

  it("refuses a host that cannot run a project from another device", () => {
    const old = host("studio", {
      protocol: { version: 8, ops: ["spawn_agent"], events: [], features: [] },
    });
    expect(option(facts([THIS_MAC, old]), "studio")).toMatchObject({
      available: false,
      reason: GATES.projectCopy.reason,
    });
  });

  it("does not ask which agents the host can run: its own composer says, once the draft is there", () => {
    const out = host("studio", { providers: [{ ...signedIn, installed: false, auth: null }] });
    expect(option(facts([THIS_MAC, out]), "studio")).toMatchObject({ available: true });
  });

  it("refuses every other device for a project with a repository no one else can clone", () => {
    const f = facts([THIS_MAC, host("studio"), host("mini")], {
      definition: definition(["git@github.com:acme/fletch.git", null]),
    });
    for (const id of ["studio", "mini"]) {
      expect(option(f, id)).toMatchObject({
        available: false,
        reason: "Gateway has no remote. Publish it first.",
      });
    }
    // The draft's own host needs no copy.
    expect(option(f, LOCAL_ENVIRONMENT_ID)?.available).toBe(true);
  });

  it("blocks nothing on the project before its definition is read", () => {
    expect(option(facts([THIS_MAC, host("studio")], { definition: null }), "studio")).toMatchObject(
      { available: true, reason: null },
    );
  });

  it("keeps the draft's own paired host open, and gates This Mac like any other device", () => {
    const f = facts([THIS_MAC, host("studio", { connection: "disconnected" })], {
      draftHost: "studio",
      definition: definition([null]),
    });
    expect(option(f, "studio")).toMatchObject({ available: true, reason: null });
    expect(option(f, LOCAL_ENVIRONMENT_ID)).toMatchObject({
      available: false,
      reason: "repo0 has no remote. Publish it first.",
    });
  });
});

describe("the words around the picker", () => {
  it("names the first repository without a remote by its label, else its folder", () => {
    expect(travelReason(definition(["https://x", null]))).toBe(
      "Gateway has no remote. Publish it first.",
    );
    expect(travelReason(definition([null]))).toBe("repo0 has no remote. Publish it first.");
    expect(travelReason(definition(["https://x"]))).toBeNull();
    expect(travelReason(null)).toBeNull();
  });

  it("names the home by the name this window knows it by", () => {
    const envs = { [LOCAL_ENVIRONMENT_ID]: THIS_MAC, studio: host("studio") };
    expect(homeName(envs, { device: "studio", name: "Mac Studio" })).toBe("Studio");
    expect(homeName(envs, { device: "gone", name: "Old Mini" })).toBe("Old Mini");
    expect(pushedNote("This Mac")).toBe(
      "Starts from the branch as pushed. Commits not pushed from This Mac are not included.",
    );
  });
});
