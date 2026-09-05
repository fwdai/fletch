/** Per-key serialization of async operations.
 *
 *  Two writers that target the same durable thing — an agent's session config, a
 *  project's setting row — must land in the order they were issued, or a slower
 *  earlier request can overwrite a faster later one and persist the opposite of
 *  what the user last chose. A queue keyed by that thing chains each op behind
 *  the previous one for the same key while leaving different keys independent.
 *
 *  Failures never block the chain: a rejected op is swallowed for the purpose
 *  of sequencing (the *caller* still sees its own rejection), so one bad write
 *  can't wedge every later one behind it. */
export interface KeyedQueue {
  /** Run `op` after every op already queued for `key`. Resolves/rejects with
   *  `op`'s own outcome. */
  run<T>(key: string, op: () => Promise<T>): Promise<T>;
  /** The tail of `key`'s chain, or undefined when nothing is pending. Awaiting
   *  it (with its rejection swallowed) is how a dependent action waits for a
   *  key's writes to settle before it acts. */
  pending(key: string): Promise<unknown> | undefined;
}

export function createKeyedQueue(): KeyedQueue {
  const tails = new Map<string, Promise<unknown>>();
  return {
    run(key, op) {
      const next = (tails.get(key) ?? Promise.resolve()).catch(() => {}).then(op);
      tails.set(key, next);
      void next
        .catch(() => {})
        .finally(() => {
          if (tails.get(key) === next) tails.delete(key);
        });
      return next;
    },
    pending(key) {
      return tails.get(key);
    },
  };
}
