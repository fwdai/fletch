/** Run `fn` once the window is past its first paint and idle.
 *
 *  `requestIdleCallback` is the right primitive and WebKit has had it since
 *  Safari 17.4, but the timeout is the point: a busy startup must not postpone
 *  a paired host indefinitely, and a runtime without the callback (a browser
 *  dev loop on an older engine) still has to get there. */
export function whenIdle(fn: () => void, timeoutMs = 2_000): void {
  const idle = (globalThis as { requestIdleCallback?: typeof requestIdleCallback })
    .requestIdleCallback;
  if (idle) idle(() => fn(), { timeout: timeoutMs });
  else setTimeout(fn, 0);
}
