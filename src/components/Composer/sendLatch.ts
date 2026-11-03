/** One send at a time across an async step before it goes out (resolving the
 *  folders a message names): while the step runs the composer is already
 *  cleared, so a second send would otherwise overtake the first, or start a
 *  second spawn from the same draft. */
export interface SendLatch {
  /** A send is between its start and its hand-off. */
  busy(): boolean;
  /** Run `send` holding the latch until it settles — it hands off inside, so
   *  nothing can go out between its step and its hand-off. `false`, with
   *  `send` not run, while another holds it. */
  run(send: () => Promise<void>): boolean;
}

export function createSendLatch(): SendLatch {
  let held = false;
  return {
    busy: () => held,
    run(send) {
      if (held) return false;
      held = true;
      void send()
        .catch(() => {})
        .finally(() => {
          held = false;
        });
      return true;
    },
  };
}
