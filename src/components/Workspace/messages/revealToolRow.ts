// The body of ToolRow's `chatFocus` effect, kept free of React so it can be
// tested without a DOM: open the row now, scroll it into view on the next
// frame, and only then consume the request.
//
// The order matters. Consuming the request flips the row's `focused` selector
// to false, which re-runs the effect and runs the previous cleanup — so a
// clear issued before the frame fires would cancel the very scroll it was
// meant to follow. Clearing from inside the frame means the cleanup can only
// ever cancel a frame that has not fired because the row went away first.

type Frame = (cb: () => void) => number;

export function revealToolRow(
  root: () => { scrollIntoView: (opts: ScrollIntoViewOptions) => void } | null,
  open: () => void,
  done: () => void,
  raf: Frame = (cb) => requestAnimationFrame(cb),
  caf: (id: number) => void = (id) => cancelAnimationFrame(id),
): () => void {
  open();
  // Next frame: the transcript's own bottom-pin effect runs after this one
  // (parent effects follow children's) and would otherwise win the scroll.
  const frame = raf(() => {
    root()?.scrollIntoView({ block: "center" });
    done();
  });
  return () => caf(frame);
}
