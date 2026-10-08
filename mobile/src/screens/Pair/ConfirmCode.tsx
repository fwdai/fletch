/** The six digits of a confirmed pairing, while the Mac decides: the same
 *  number its prompt shows, which is the whole check — they match only if
 *  nothing sits between the two. */
export function ConfirmCode({ code }: { code: string }) {
  return (
    <div className="confirm-code">
      <span className="label">Your Mac shows</span>
      <div className="digits">
        {code.slice(0, 3)} {code.slice(3)}
      </div>
      <p>Check the numbers match, then click Accept on your Mac.</p>
    </div>
  );
}
