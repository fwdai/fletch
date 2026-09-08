/** Resolve to `fallback` if `work` has not settled within `ms`. For best-effort
 *  steps that must never hold up what comes after them: the remote client has
 *  no request timeout of its own, so a host that stops answering without
 *  closing the socket would otherwise stall the caller for good. */
export function withTimeout<T, F = undefined>(
  work: Promise<T>,
  ms: number,
  fallback?: F,
): Promise<T | F> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => resolve(fallback as F), ms);
    work.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}
