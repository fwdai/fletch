import { describe, expect, it } from "vitest";
import { isValidRepoName, parseRepoSpec } from "@/util/repoSpec";

describe("parseRepoSpec", () => {
  it("parses owner/repo", () => {
    expect(parseRepoSpec("octocat/hello")).toEqual({ valid: true, name: "hello" });
  });

  it("parses an https url", () => {
    expect(parseRepoSpec("https://github.com/octocat/Hello-World")).toEqual({
      valid: true,
      name: "Hello-World",
    });
  });

  it("strips a .git suffix and trailing slash", () => {
    expect(parseRepoSpec("https://github.com/octocat/Hello-World.git/")).toEqual({
      valid: true,
      name: "Hello-World",
    });
  });

  it("parses an ssh url", () => {
    expect(parseRepoSpec("git@github.com:octocat/my_repo.git")).toEqual({
      valid: true,
      name: "my_repo",
    });
  });

  it("rejects empty input", () => {
    expect(parseRepoSpec("   ").valid).toBe(false);
  });

  it("rejects a tail with illegal characters", () => {
    expect(parseRepoSpec("owner/bad name").valid).toBe(false);
  });

  it("rejects a tail starting with a hyphen (gh flag injection)", () => {
    expect(parseRepoSpec("owner/-foo").valid).toBe(false);
  });
});

describe("isValidRepoName", () => {
  it("accepts typical names", () => {
    for (const ok of ["my-app", "my_app", "App.2", "x"]) {
      expect(isValidRepoName(ok)).toBe(true);
    }
  });

  it("rejects bad names", () => {
    for (const bad of ["", "  ", ".", "..", "a/b", "a b", "a:b", "café", "-foo", "--push"]) {
      expect(isValidRepoName(bad)).toBe(false);
    }
  });
});
