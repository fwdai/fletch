import type { ComponentPropsWithoutRef } from "react";

/** Text size is baked in (`.text-base`) so callers don't restate it — see the
 *  type convention in styles/shared/type.css. */
const skin = (...parts: (string | false | undefined)[]) =>
  ["ui-input", "text-base", ...parts].filter(Boolean).join(" ");

/** Monospace content — tokens, URLs, commands, JSON. */
interface MonoOption {
  mono?: boolean;
}

type TextInputProps = ComponentPropsWithoutRef<"input"> &
  MonoOption & {
    /** Tint the border red — a validation failure the caller has already decided. */
    invalid?: boolean;
  };

/** Single-line text field. The app's one input appearance; everything else
 *  (value, onChange, type, placeholder, …) passes straight through. */
export function TextInput({ mono, invalid, className, ...rest }: TextInputProps) {
  return <input className={skin(mono && "mono", invalid && "invalid", className)} {...rest} />;
}

type TextAreaProps = ComponentPropsWithoutRef<"textarea"> & MonoOption;

/** Multi-line sibling of `TextInput`. Prose boxes layer their own height and
 *  padding on top via `className` (e.g. `.ca-textarea`, `.fb-textarea`). */
export function TextArea({ mono, className, ...rest }: TextAreaProps) {
  return <textarea className={skin(mono && "mono", className)} {...rest} />;
}
