import { useEffect, useState } from "react";
import { Icon } from "@/components/Icon";

interface AvatarProps {
  /** A `data:` URI (or URL) for the avatar image, or null for none. */
  avatarUrl: string | null;
  /** Shown when there is no image, or if it fails to load. Empty when nothing
   *  is known about the user: a neutral person glyph stands in then. */
  initials: string;
  /** Sizing/shape class for the wrapper (e.g. "user-avatar", "set-avatar"). */
  className: string;
  /** The glyph's size, for the no-identity case; scale it with the wrapper. */
  glyphSize?: number;
  alt?: string;
}

/** Renders an account avatar image when available, falling back to initials,
 *  and to a person glyph when there are none. Shared by the sidebar footer and
 *  the account settings pane so both stay in sync. The `onError` fallback
 *  covers a malformed data URI or a stale row that still holds an unreachable
 *  remote URL. */
export function Avatar({ avatarUrl, initials, className, glyphSize = 13, alt = "" }: AvatarProps) {
  const [failed, setFailed] = useState(false);
  // Re-attempt loading when the source changes (e.g. after re-login). avatarUrl
  // isn't read in the body, but it's the intended trigger — the effect exists to
  // reset `failed` whenever the source changes, so it must stay in the deps.
  // biome-ignore lint/correctness/useExhaustiveDependencies: avatarUrl is the intended re-run trigger, not an unused dep
  useEffect(() => setFailed(false), [avatarUrl]);

  let body: React.ReactNode;
  if (avatarUrl && !failed) {
    body = <img src={avatarUrl} alt={alt} draggable={false} onError={() => setFailed(true)} />;
  } else if (initials) {
    body = initials;
  } else {
    body = <Icon name="user" size={glyphSize} strokeWidth={1.8} />;
  }
  return <div className={className}>{body}</div>;
}
