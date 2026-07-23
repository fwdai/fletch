/** Time-of-day greeting for the Home hero. Pure so it can be unit-tested; the
 *  caller passes the current Date (the component reads `new Date()` at render).
 *  Buckets: 05:00–11:59 morning, 12:00–17:59 afternoon, otherwise evening. */
export function greeting(now: Date): string {
  const h = now.getHours();
  if (h >= 5 && h < 12) return "Good morning";
  if (h >= 12 && h < 18) return "Good afternoon";
  return "Good evening";
}
