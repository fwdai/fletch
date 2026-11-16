// @vitest-environment happy-dom
// The one rule that decides whether an agent can be spawned. Two things are
// being pinned here: that the machine the agent would run on is what picks
// which facts apply, and that a refusal reaches an acting control as a whole
// sentence — the send button and ↵ share this rule, so a gap in it is a draft
// that the button refuses and Enter sends anyway.

import { describe, expect, it } from "vitest";
import type { HostProvider } from "@/remote/types";
import { useAppStore } from "@/store";
import type { EnvironmentEntry } from "@/store/environments";
import { renderHook } from "@/test/render";
import {
  type AvailabilityFacts,
  agentAvailability,
  blockedSentence,
  useAgentAvailability,
} from "./availability";

const local: EnvironmentEntry = {
  id: "local",
  name: "This Mac",
  kind: "local",
  connection: "connected",
};

const provider = (over: Partial<HostProvider> & { id: string }): HostProvider => ({
  label: over.id,
  installed: true,
  version: "1.0.0",
  auth: "signed_in",
  loginCommand: `${over.id} login`,
  ...over,
});

const host = (providers?: HostProvider[]): EnvironmentEntry => ({
  id: "host-key-1",
  name: "Cloud box",
  kind: "remote",
  connection: "connected",
  protocol: { version: 2, ops: ["host_providers"], events: [], features: [] },
  ...(providers ? { providers } : {}),
});

/** This Mac, claude and codex installed — the case where nothing is blocked,
 *  so a test that blocks has said why itself. */
const facts = (over: Partial<AvailabilityFacts> = {}): AvailabilityFacts => ({
  env: local,
  providerPaths: { claude: "/usr/local/bin/claude", codex: "/usr/local/bin/codex" },
  providerVersions: { claude: "2.1.4", codex: "0.48.0" },
  providersProbed: true,
  ...over,
});

describe("agentAvailability, on This Mac", () => {
  it("offers an installed provider, with its probed version", () => {
    const a = agentAvailability(facts(), "claude");
    expect(a.reason).toBeNull();
    expect(a.fix).toBeNull();
    expect(a.note).toBe("2.1.4");
    expect(a.installed).toBe(true);
  });

  it("blocks a provider the probe did not find, and marks it absent", () => {
    const a = agentAvailability(facts(), "cursor");
    expect(a.reason).toBe("Not installed — see Settings › Providers");
    expect(a.note).toBe("Not installed");
    expect(a.installed).toBe(false);
  });

  it("fails open until the probe has actually run", () => {
    // A transient detection failure must never disable an agent the user has.
    const a = agentAvailability(facts({ providersProbed: false }), "cursor");
    expect(a.reason).toBeNull();
    expect(a.installed).toBe(true);
  });
});

describe("agentAvailability, on a paired host", () => {
  it("offers a provider the host has and is signed into", () => {
    const a = agentAvailability(facts({ env: host([provider({ id: "claude" })]) }), "claude");
    expect(a.reason).toBeNull();
    // The host's version, not this Mac's — the agent runs there.
    expect(a.note).toBe("1.0.0");
  });

  it("ignores this Mac's provider probe entirely", () => {
    // Nothing is installed here; everything is installed there.
    const env = host([provider({ id: "cursor" })]);
    const a = agentAvailability(
      facts({ env, providerPaths: {}, providerVersions: {}, providersProbed: true }),
      "cursor",
    );
    expect(a.reason).toBeNull();
  });

  it("blocks a provider the host has not got, and marks it absent", () => {
    const env = host([provider({ id: "cursor", installed: false, version: null, auth: null })]);
    const a = agentAvailability(facts({ env }), "cursor");
    expect(a.reason).toBe("Not installed on Cloud box");
    expect(a.note).toBe("Not installed");
    expect(a.installed).toBe(false);
  });

  it("blocks a provider the host is signed out of", () => {
    const env = host([provider({ id: "claude", auth: "signed_out" })]);
    const a = agentAvailability(facts({ env }), "claude");
    expect(a.reason).toBe(
      "Not signed in on Cloud box — run `fletch-host provider login claude` there",
    );
    expect(a.note).toBe("Signed out");
    expect(a.installed).toBe(true);
  });

  it("blocks nothing while the host has said nothing", () => {
    // Not greeted yet, or too old for the op: unknown is not a refusal, and
    // this Mac's probe must not stand in for one.
    const a = agentAvailability(facts({ env: host(), providerPaths: {} }), "antigravity");
    expect(a.reason).toBeNull();
    expect(a.note).toBe("");
    expect(a.installed).toBe(true);
  });
});

describe("blockedSentence", () => {
  it("is null for an agent that can be spawned", () => {
    expect(blockedSentence(agentAvailability(facts(), "claude"))).toBeNull();
  });

  it("carries the reason and what to do about it", () => {
    expect(blockedSentence(agentAvailability(facts(), "cursor"))).toBe(
      "Not installed — see Settings › Providers — install it in Settings › Providers",
    );
  });

  it("adds nothing to a host refusal, which already ends in its own remedy", () => {
    const env = host([provider({ id: "claude", auth: "signed_out" })]);
    expect(blockedSentence(agentAvailability(facts({ env }), "claude"))).toBe(
      "Not signed in on Cloud box — run `fletch-host provider login claude` there",
    );
  });
});

describe("useAgentAvailability", () => {
  // A host that has only codex, driven from a Mac that has only claude.
  const seed = () => {
    const before = useAppStore.getState();
    useAppStore.setState({
      environments: { local, [remote.id]: remote },
      activeEnvironmentId: remote.id,
      providerPaths: { claude: "/usr/local/bin/claude" },
      providerVersions: { claude: "1.0.0" },
      providersProbed: true,
    });
    return () => useAppStore.setState(before, true);
  };
  const remote = host([
    provider({ id: "codex" }),
    provider({ id: "claude", installed: false, version: null }),
  ]);

  it("answers for the active environment by default", () => {
    const restore = seed();
    const { result } = renderHook(() => useAgentAvailability());
    expect(result.current("claude").installed).toBe(false);
    expect(result.current("codex").installed).toBe(true);
    restore();
  });

  it("answers for This Mac when asked, whichever host is active", () => {
    const restore = seed();
    const { result } = renderHook(() => useAgentAvailability("local"));
    expect(result.current("claude").installed).toBe(true);
    expect(result.current("codex").installed).toBe(false);
    restore();
  });
});
