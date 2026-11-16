// @vitest-environment happy-dom
import { describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@/test/render";
import { useCachedList } from "./useCachedList";

/** A `list` whose answer the test delivers by hand. */
function deferred<T>() {
  let resolve: (value: T[]) => void = () => {};
  let reject: (reason: unknown) => void = () => {};
  const list = vi.fn(
    () =>
      new Promise<T[]>((res, rej) => {
        resolve = res;
        reject = rej;
      }),
  );
  return { list, resolve: (v: T[]) => resolve(v), reject: (e: unknown) => reject(e) };
}

describe("useCachedList", () => {
  it("is null until the first answer, and fetches only once open", async () => {
    const { list, resolve } = deferred<string>();
    const hook = renderHook(({ open }) => useCachedList(list, open), {
      initialProps: { open: false },
    });
    expect(list).not.toHaveBeenCalled();

    hook.rerender({ open: true });
    expect(list).toHaveBeenCalledTimes(1);
    expect(hook.result.current).toBeNull();

    await act(async () => resolve(["a"]));
    expect(hook.result.current).toEqual(["a"]);
  });

  it("keeps the answer of a fetch its panel closed on", async () => {
    const { list, resolve } = deferred<string>();
    const hook = renderHook(({ open }) => useCachedList(list, open), {
      initialProps: { open: true },
    });
    // The pointer moves on before the list answers, and comes back within the
    // cache window: no second fetch, so the first one's answer has to land.
    hook.rerender({ open: false });
    await act(async () => resolve(["a"]));
    hook.rerender({ open: true });

    expect(list).toHaveBeenCalledTimes(1);
    expect(hook.result.current).toEqual(["a"]);
  });

  it("reads a failed first fetch as empty, and keeps the list through a failed refresh", async () => {
    vi.useFakeTimers();
    try {
      const first = deferred<string>();
      const hook = renderHook(({ list, open }) => useCachedList(list, open), {
        initialProps: { list: first.list, open: true },
      });
      await act(async () => first.reject(new Error("offline")));
      expect(hook.result.current).toEqual([]);

      const second = deferred<string>();
      hook.rerender({ list: second.list, open: false });
      vi.advanceTimersByTime(16_000);
      hook.rerender({ list: second.list, open: true });
      await act(async () => second.resolve(["a"]));
      expect(hook.result.current).toEqual(["a"]);

      const third = deferred<string>();
      hook.rerender({ list: third.list, open: false });
      vi.advanceTimersByTime(16_000);
      hook.rerender({ list: third.list, open: true });
      await act(async () => third.reject(new Error("offline")));
      expect(hook.result.current).toEqual(["a"]);
    } finally {
      vi.useRealTimers();
    }
  });
});
