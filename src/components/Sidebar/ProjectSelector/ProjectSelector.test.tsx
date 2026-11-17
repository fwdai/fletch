// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import { render, screen } from "@/test/render";
import type { ProjectSignal } from "../projectSignal";
import { type ProjectOption, ProjectSelector } from ".";

const project = (key: string, signal: ProjectSignal | null = null): ProjectOption => ({
  key,
  label: key,
  icon: { id: key, name: key } as ProjectOption["icon"],
  count: 1,
  signal,
});

const trigger = () => screen.getByRole("button", { name: /^Projects shown/ });

describe("ProjectSelector", () => {
  it("says all projects are shown while there is no focus", () => {
    render(
      <ProjectSelector projects={[project("a"), project("b")]} focus={[]} onChange={() => {}} />,
    );
    expect(trigger().getAttribute("data-tip")).toBe("All projects");
    expect(trigger().classList.contains("active")).toBe(false);
  });

  it("counts the focused projects and lights up", () => {
    render(
      <ProjectSelector
        projects={[project("a"), project("b"), project("c")]}
        focus={["a", "b"]}
        onChange={() => {}}
      />,
    );
    expect(trigger().getAttribute("data-tip")).toBe("2 of 3 projects");
    expect(trigger().classList.contains("active")).toBe(true);
  });

  it("reports a signal from a hidden project, not from a shown one", () => {
    const projects = [project("a", "running"), project("b", "waiting")];
    const { rerender } = render(
      <ProjectSelector projects={projects} focus={["a"]} onChange={() => {}} />,
    );
    expect(trigger().getAttribute("data-tip")).toBe(
      "1 of 2 projects\nWaiting for input in a hidden project",
    );

    rerender(<ProjectSelector projects={projects} focus={[]} onChange={() => {}} />);
    expect(trigger().getAttribute("data-tip")).toBe("All projects");
  });
});
