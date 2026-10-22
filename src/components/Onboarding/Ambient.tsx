// The stage behind both screens: one slow-breathing copper glow, a vignette
// and a film of grain. The glow sits high and centered behind the welcome
// lockup and slides aside on the pre-flight screen so the panel on the right
// has light behind it. `phase` is the screen index.

export function Ambient({ phase }: { phase: number }) {
  return (
    <div className="ob-ambient">
      <div className={`ob-glow ${phase > 0 ? "aside" : ""}`} />
      <div className="ob-vignette" />
      <div className="ob-grain" />
    </div>
  );
}
