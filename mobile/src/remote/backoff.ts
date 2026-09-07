/** Reconnect delay schedule from docs/remote-protocol.md: 1 s, 2 s, 4 s … 30 s.
 *  `attempt` is 0-based. Exported on its own so the schedule is testable
 *  without driving a socket. */
export function backoffDelay(attempt: number, base = 1000, cap = 30_000): number {
  return Math.min(cap, base * 2 ** Math.max(0, attempt));
}
