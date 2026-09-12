import { describe, expect, it, vi } from "vitest";
import { attachSwipe, shouldCommit } from "../src/lib/swipe";

describe("shouldCommit", () => {
  it("commits a slow drag once it has covered enough of the element", () => {
    expect(shouldCommit(100, 400, 0)).toBe(false);
    expect(shouldCommit(140, 400, 0)).toBe(true);
  });
  it("commits a flick however short, and refuses a long drag flung back", () => {
    expect(shouldCommit(30, 400, 0.6)).toBe(true);
    expect(shouldCommit(300, 400, -0.5)).toBe(false);
  });
});

/** jsdom has no `Touch`; a plain event with a `touches` array is all the
 *  gesture reads. */
function touch(type: string, x: number, y: number, target: Element) {
  const e = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(e, "touches", { value: [{ clientX: x, clientY: y }] });
  target.dispatchEvent(e);
  return e;
}

function mount(width = 400, height = 800) {
  const target = document.createElement("div");
  const inner = document.createElement("div");
  target.appendChild(inner);
  document.body.appendChild(target);
  Object.defineProperty(target, "offsetWidth", { value: width });
  Object.defineProperty(target, "offsetHeight", { value: height });
  target.getBoundingClientRect = () =>
    ({ left: 0, top: 0, right: width, bottom: height, width, height, x: 0, y: 0 }) as DOMRect;
  return { target, inner };
}

describe("attachSwipe", () => {
  it("drives --swipe from an edge drag and pops on release", () => {
    const { target, inner } = mount();
    const onCommit = vi.fn();
    let t = 0;
    attachSwipe(target, target, () => ({ axis: "x", edge: 28, onCommit, now: () => t }));

    touch("touchstart", 10, 300, inner);
    t = 50;
    const move = touch("touchmove", 60, 302, inner);
    expect(move.defaultPrevented).toBe(true);
    expect(target.classList.contains("dragging")).toBe(true);
    t = 200;
    touch("touchmove", 210, 305, inner);
    expect(target.style.getPropertyValue("--swipe")).toBe("0.5");

    touch("touchend", 210, 305, inner);
    expect(target.classList.contains("dragging")).toBe(false);
    expect(target.style.getPropertyValue("--swipe")).toBe("");
    expect(onCommit).toHaveBeenCalledOnce();
  });

  it("snaps back without committing when the drag falls short", () => {
    const { target, inner } = mount();
    const onCommit = vi.fn();
    let t = 0;
    attachSwipe(target, target, () => ({ axis: "x", edge: 28, onCommit, now: () => t }));
    touch("touchstart", 10, 300, inner);
    for (const x of [20, 30, 40, 50, 60]) {
      t += 100;
      touch("touchmove", x, 300, inner);
    }
    touch("touchend", 60, 300, inner);
    expect(onCommit).not.toHaveBeenCalled();
    expect(target.style.getPropertyValue("--swipe")).toBe("");
  });

  it("ignores touches that start away from the edge", () => {
    const { target, inner } = mount();
    const onCommit = vi.fn();
    attachSwipe(target, target, () => ({ axis: "x", edge: 28, onCommit }));
    touch("touchstart", 100, 300, inner);
    const move = touch("touchmove", 300, 300, inner);
    expect(move.defaultPrevented).toBe(false);
    touch("touchend", 300, 300, inner);
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("yields to a scroll: the first move decides the axis", () => {
    const { target, inner } = mount();
    const onCommit = vi.fn();
    attachSwipe(target, target, () => ({ axis: "x", edge: 28, onCommit }));
    touch("touchstart", 10, 300, inner);
    const move = touch("touchmove", 14, 330, inner);
    expect(move.defaultPrevented).toBe(false);
    // Later horizontal movement in the same touch no longer counts.
    touch("touchmove", 300, 330, inner);
    touch("touchend", 300, 330, inner);
    expect(onCommit).not.toHaveBeenCalled();
    expect(target.classList.contains("dragging")).toBe(false);
  });

  it("leaves a scrolled sheet body to scroll back first", () => {
    const { target, inner } = mount();
    const onCommit = vi.fn();
    Object.defineProperty(inner, "scrollTop", { value: 120 });
    attachSwipe(target, target, () => ({ axis: "y", onCommit }));
    touch("touchstart", 100, 100, inner);
    const move = touch("touchmove", 100, 400, inner);
    expect(move.defaultPrevented).toBe(false);
    touch("touchend", 100, 400, inner);
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("stays put while disabled", () => {
    const { target, inner } = mount();
    const onCommit = vi.fn();
    attachSwipe(target, target, () => ({ axis: "x", edge: 28, enabled: false, onCommit }));
    touch("touchstart", 10, 300, inner);
    touch("touchmove", 300, 300, inner);
    touch("touchend", 300, 300, inner);
    expect(onCommit).not.toHaveBeenCalled();
  });
});
