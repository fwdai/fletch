// The mouse moves the palette's cursor only when it actually moves. A
// keyboard step scrolls the list under a resting pointer, and the browser
// then fires a mousemove with the same coordinates as before; that must not
// snatch the cursor back. Sampled on the whole list, not per row, so the
// pointer is known wherever it rests — over the active row, over a section
// label, over empty space — before the first keyboard step.

export interface Point {
  x: number;
  y: number;
}

/** Whether `next` is a real move from `last`. With no sample yet it is not:
 *  the first event only seeds the sample, so a pointer that was resting over
 *  the list when the palette opened cannot claim the cursor on a scroll. */
export function pointerMoved(last: Point | null, next: Point): boolean {
  return last !== null && (last.x !== next.x || last.y !== next.y);
}
