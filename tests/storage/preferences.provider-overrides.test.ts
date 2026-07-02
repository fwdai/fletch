import { describe, expect, it } from "vitest";

import { parseProviderPathOverrides } from "@/storage/preferences";

describe("parseProviderPathOverrides", () => {
  it("extracts agent_bin_path_<id> rows and strips the prefix", () => {
    const overrides = parseProviderPathOverrides({
      theme: "dark",
      agent_bin_path_claude: "/opt/homebrew/bin/claude",
      agent_bin_path_cursor: "~/bin/cursor-agent",
      providers: "{}",
    });
    expect(overrides).toEqual({
      claude: "/opt/homebrew/bin/claude",
      cursor: "~/bin/cursor-agent",
    });
  });

  it("drops blank values (a cleared override) and ignores unrelated keys", () => {
    const overrides = parseProviderPathOverrides({
      agent_bin_path_codex: "   ",
      agent_bin_path_pi: "",
      density: "comfortable",
    });
    expect(overrides).toEqual({});
  });

  it("returns an empty map when there are no overrides", () => {
    expect(parseProviderPathOverrides({})).toEqual({});
  });
});
