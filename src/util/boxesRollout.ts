/** The rollout rule behind `useBoxesUnlocked`, without the store: a dev build,
 *  or an admin in production. Kept apart so the store's own slices can apply
 *  the same rule outside React. */
export function boxesUnlockedFor(admin: boolean): boolean {
  return import.meta.env.DEV || admin;
}
