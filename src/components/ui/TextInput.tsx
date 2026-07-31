import type { ComponentPropsWithoutRef } from "react";

/** Shared bits of both fields. Text size is baked in (`.text-base`) so callers
 *  don't restate it — see the type convention in styles/shared/type.css. */
interface Skin {
  /** Monospace content — tokens, URLs, commands, JSON. */
  mono?: boolean;
}

function skinClass({ mono }: Skin, extra: string | undefined, invalid?: boolean) {
  return ["ui-input", "text-base", mono ? "mono" : "", invalid ? "invalid" : "", extra]
    .filter(Boolean)
    .join(" ");
}

type TextInputProps = Skin & {
  /** Tint the border red — a failed validation the caller has already decided. */
  invalid?: boolean;
} & Omit<ComponentPropsWithoutRef<"input">, keyof Skin | "invalid">;

/** Single-line text field. The app's one input appearance; everything else
 *  (value, onChange, type, placeholder, …) passes straight through. */
export function TextInput({ mono, invalid, className, ...rest }: TextInputProps) {
  return <input className={skinClass({ mono }, className, invalid)} {...rest} />;
}

type TextAreaProps = Skin & Omit<ComponentPropsWithoutRef<"textarea">, keyof Skin>;

/** Multi-line sibling of `TextInput`. Prose boxes layer their own height and
 *  padding on top via `className` (e.g. `.ca-textarea`, `.fb-textarea`). */
export function TextArea({ mono, className, ...rest }: TextAreaProps) {
  return <textarea className={skinClass({ mono }, className)} {...rest} />;
}
