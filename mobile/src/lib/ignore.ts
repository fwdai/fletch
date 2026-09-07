/** Deliberately drop a rejection. Store actions record their failure in
 *  `lastError` and rethrow so a caller that cares can react; a fire-and-forget
 *  call site says so with `.catch(ignore)` instead of leaving an unhandled
 *  rejection behind. */
export const ignore = (): void => {};
