import { useEffect, useState } from "react";

interface AvatarProps {
  /** A `data:` URI (or URL) for the avatar image, or null for none. */
  avatarUrl: string | null;
  /** Shown when there is no image, or if it fails to load. */
  initials: string;
  /** Sizing/shape class for the wrapper (e.g. "user-avatar", "set-avatar"). */
  className: string;
  alt?: string;
}

/** Renders an account avatar image when available, falling back to initials.
 *  Shared by the sidebar footer and the account settings pane so both stay in
 *  sync. The `onError` fallback covers a malformed data URI or a stale row that
 *  still holds an unreachable remote URL. */
export function Avatar({ avatarUrl, initials, className, alt = "" }: AvatarProps) {
  const [failed, setFailed] = useState(false);
  // Re-attempt loading when the source changes (e.g. after re-login). avatarUrl
  // isn't read in the body, but it's the intended trigger — the effect exists to
  // reset `failed` whenever the source changes, so it must stay in the deps.
  // biome-ignore lint/correctness/useExhaustiveDependencies: avatarUrl is the intended re-run trigger, not an unused dep
  useEffect(() => setFailed(false), [avatarUrl]);

  return (
    <div className={className}>
      {avatarUrl && !failed ? (
        <img src={avatarUrl} alt={alt} draggable={false} onError={() => setFailed(true)} />
      ) : (
        initials
      )}
    </div>
  );
}
