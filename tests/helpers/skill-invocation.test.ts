import { describe, expect, it } from "vitest";
import { invocableSkills, resolveSkillInvocation } from "@/helpers";
import { type Skill, skillSlug } from "@/storage/skills";

function skill(over: Partial<Skill> & Pick<Skill, "name">): Skill {
  return {
    id: `sk-${over.name}`,
    description: "",
    body: "# body",
    created_at: 0,
    updated_at: 0,
    ...over,
  };
}

describe("skillSlug", () => {
  it("lowercases and collapses non-alphanumerics, matching the backend slug", () => {
    expect(skillSlug("Code Review")).toBe("code-review");
    expect(skillSlug("  release -- v2  ")).toBe("release-v2");
    expect(skillSlug("Déjà vu")).toBe("d-j-vu");
  });

  it("falls back to 'skill' when nothing usable remains", () => {
    expect(skillSlug("!!!")).toBe("skill");
    expect(skillSlug("")).toBe("skill");
  });
});

describe("invocableSkills", () => {
  it("maps each skill to its slugged command", () => {
    const out = invocableSkills([skill({ name: "Code Review" })], "codex");
    expect(out).toHaveLength(1);
    expect(out[0].command).toBe("code-review");
  });

  it("drops a skill whose slug collides with a provider command", () => {
    // "help" is a claude built-in; the provider command wins the name.
    const out = invocableSkills([skill({ name: "Help" }), skill({ name: "Release" })], "claude");
    expect(out.map((s) => s.command)).toEqual(["release"]);
  });

  it("keeps only the first of two skills sharing a slug", () => {
    const first = skill({ name: "Ship It", body: "first" });
    const second = skill({ name: "ship-it", body: "second" });
    const out = invocableSkills([first, second], "codex");
    expect(out).toHaveLength(1);
    expect(out[0].skill.body).toBe("first");
  });
});

describe("resolveSkillInvocation", () => {
  const skills = [skill({ name: "Code Review", description: "Review the diff", body: "# steps" })];

  it("resolves a matching command into a snapshot and follow-it-now prompt", () => {
    const res = resolveSkillInvocation(skills, "codex", "/code-review");
    expect(res).not.toBeNull();
    expect(res?.snapshot).toEqual({
      name: "Code Review",
      description: "Review the diff",
      body: "# steps",
    });
    expect(res?.prompt).toContain('"Code Review"');
    expect(res?.prompt).not.toContain("Arguments:");
  });

  it("carries everything after the command as arguments, newlines included", () => {
    const res = resolveSkillInvocation(skills, "codex", "/code-review focus on auth\nand tests");
    expect(res?.prompt).toContain("Arguments: focus on auth\nand tests");
  });

  it("ignores plain messages, unknown commands, and a bare slash", () => {
    expect(resolveSkillInvocation(skills, "codex", "review this")).toBeNull();
    expect(resolveSkillInvocation(skills, "codex", "/nope")).toBeNull();
    expect(resolveSkillInvocation(skills, "codex", "/")).toBeNull();
  });

  it("lets a provider command win a name clash (no skill invocation)", () => {
    // A skill named "compact" slugs onto claude's built-in /compact; the
    // built-in keeps the name, so the text passes through as a command.
    const clashing = [skill({ name: "compact" })];
    expect(resolveSkillInvocation(clashing, "claude", "/compact")).toBeNull();
  });
});
