/** Grow a textarea to fit its content, between `min` and `max` pixels. Called
 *  imperatively from the change handler rather than from an effect, so the
 *  measurement happens exactly when the text changes. */
export function autosize(el: HTMLTextAreaElement | null, min = 0, max = 120): void {
  if (!el) return;
  el.style.height = "auto";
  el.style.height = `${Math.min(max, Math.max(min, el.scrollHeight))}px`;
}
