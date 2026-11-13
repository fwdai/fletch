// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import { render, screen } from "@/test/render";
import { RunPorts } from "./RunPorts";

const web = { guest_port: 3000, port: 3001 };
const postgres = { guest_port: 5432, port: 5433 };
const redis = { guest_port: 6379, port: 6379 };

describe("RunPorts", () => {
  it("renders nothing when the run's own port is the only one", () => {
    const { container } = render(<RunPorts ports={[web]} runGuestPort={3000} />);
    expect(container.innerHTML).toBe("");
  });

  it("links every other port to its port on this Mac", () => {
    render(<RunPorts ports={[web, postgres, redis]} runGuestPort={3000} />);
    const links = screen.getAllByRole("link");
    expect(links.map((a) => a.getAttribute("href"))).toEqual([
      "http://localhost:5433",
      "http://localhost:6379",
    ]);
    expect(links.map((a) => a.textContent)).toEqual([":5432→ localhost:5433", ":6379"]);
  });

  it("lists every port while the run has none of its own", () => {
    render(<RunPorts ports={[web, redis]} runGuestPort={null} />);
    expect(screen.getAllByRole("link")).toHaveLength(2);
  });
});
