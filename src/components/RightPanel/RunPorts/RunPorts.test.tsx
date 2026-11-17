// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import { fireEvent, render, screen } from "@/test/render";
import { RunPorts } from "./index";

const web = { guest_port: 3000, port: 3001, run: true };
const worker = { guest_port: 8787, port: 8788, run: true };
const inspector = { guest_port: 9229, port: 9229, run: true };
const postgres = { guest_port: 5432, port: 5433, run: false };
const agentServer = { guest_port: 4567, port: 4567, run: false };

const hrefs = (role: "link" | "menuitem") =>
  screen.getAllByRole(role).map((a) => a.getAttribute("href"));

describe("RunPorts", () => {
  it("renders nothing when the run's own port is the only one", () => {
    const { container } = render(<RunPorts ports={[web]} runGuestPort={3000} />);
    expect(container.innerHTML).toBe("");
  });

  it("links every other port the run listens on to its port on this Mac", () => {
    render(<RunPorts ports={[web, worker, inspector]} runGuestPort={3000} />);
    expect(hrefs("link")).toEqual(["http://localhost:8788", "http://localhost:9229"]);
    expect(screen.getAllByRole("link").map((a) => a.textContent)).toEqual([
      ":8787→ localhost:8788",
      ":9229",
    ]);
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("keeps the ports the run does not listen on behind a menu", () => {
    render(<RunPorts ports={[web, worker, postgres, agentServer]} runGuestPort={3000} />);
    expect(hrefs("link")).toEqual(["http://localhost:8788"]);
    // An icon alone: the count is its name and tip, not text in the bar.
    const trigger = screen.getByRole("button", { name: "2 other ports" });
    expect(trigger.textContent).toBe("");
    expect(screen.queryByRole("menu")).toBeNull();

    fireEvent.click(trigger);
    expect(hrefs("menuitem")).toEqual(["http://localhost:5433", "http://localhost:4567"]);
    expect(screen.getAllByRole("menuitem").map((a) => a.textContent)).toEqual([
      ":5432→ localhost:5433",
      ":4567",
    ]);

    fireEvent.click(screen.getAllByRole("menuitem")[0]);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("lists a server started while the run has no port of its own behind the menu", () => {
    render(<RunPorts ports={[agentServer]} runGuestPort={null} />);
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.getByRole("button", { name: "1 other port" })).toBeTruthy();
  });

  it("takes a port that does not say who listens for one of the others", () => {
    render(<RunPorts ports={[{ guest_port: 6379, port: 6379 }]} runGuestPort={null} />);
    expect(screen.queryByRole("link")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "1 other port" }));
    expect(hrefs("menuitem")).toEqual(["http://localhost:6379"]);
  });
});
