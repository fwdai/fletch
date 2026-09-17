import { open } from "@tauri-apps/plugin-shell";
import { type ImgHTMLAttributes, useMemo, useState } from "react";
import type { Components } from "react-markdown";
import type { PrComment } from "@/api";
import { Icon } from "@/components/Icon";
import { Markdown } from "@/components/Markdown";
import { commentLocation } from "@/components/RightPanel/prComments";
import { htmlToMarkdown, markdownImages, markdownToText } from "@/util/markdownText";

/** A preview longer than this (roughly two panel lines) gets a "Show more". */
const PREVIEW_CHARS = 140;

const openExternal = (url: string) => void open(url).catch(() => {});

/** Images in an expanded comment: a bounded thumbnail that opens in the
 *  browser on click. GitHub's attachment URLs need a signed-in session for
 *  private repos, so a load failure degrades to a link rather than a broken
 *  image glyph. */
function CommentImage({ src, alt }: ImgHTMLAttributes<HTMLImageElement>) {
  const [failed, setFailed] = useState(false);
  if (typeof src !== "string" || !src) return null;
  const label = alt || "image";
  if (failed) {
    return (
      <button
        type="button"
        className="pc-img-link"
        onClick={() => openExternal(src)}
        aria-label={`Open ${label} on GitHub`}
      >
        <Icon name="image" size={11} />
        <span>{label}</span>
        <Icon name="external" size={10} />
      </button>
    );
  }
  return (
    <button
      type="button"
      className="pc-img"
      onClick={() => openExternal(src)}
      aria-label={`Open ${label} in browser`}
    >
      <img src={src} alt={alt ?? ""} loading="lazy" onError={() => setFailed(true)} />
    </button>
  );
}

const MD_COMPONENTS: Components = { img: CommentImage };

export function CommentRow({
  comment: c,
  onAddToChat,
}: {
  comment: PrComment;
  onAddToChat: (c: PrComment) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  // GitHub bodies mix markdown with HTML (pasted screenshots, bot wrappers).
  // Normalise to markdown once; the preview flattens that to prose.
  const md = useMemo(() => htmlToMarkdown(c.body), [c.body]);
  const text = useMemo(() => markdownToText(md), [md]);
  const imageCount = useMemo(() => markdownImages(md).length, [md]);
  const loc = commentLocation(c);
  // Anything the one-line preview can't show faithfully is worth expanding:
  // long prose, images, or a code block (a bot's suggested fix, typically).
  const expandable = text.length > PREVIEW_CHARS || imageCount > 0 || /```|~~~/.test(md);

  return (
    <div className={expanded ? "pr-comment is-open" : "pr-comment"}>
      <Icon name={c.is_bot ? "bot" : "user"} size={12} />
      <div className="pc-body">
        <div className="pc-top text-xs">
          <span className="pc-author">{c.author}</span>
          {loc && <span className="pc-loc text-xs">{loc}</span>}
          {c.replies > 0 && (
            <span className="pc-replies">
              +{c.replies} {c.replies === 1 ? "reply" : "replies"}
            </span>
          )}
        </div>
        {expanded ? (
          <div className="pc-md text-sm">
            <Markdown components={MD_COMPONENTS}>{md}</Markdown>
          </div>
        ) : (
          <div className="pc-text text-sm">{text || (imageCount > 0 ? "(image)" : "")}</div>
        )}
        <div className="pc-acts">
          <button
            type="button"
            className="pc-act pc-act--primary"
            aria-label="Add comment to chat"
            onClick={() => onAddToChat(c)}
          >
            <Icon name="chatPlus" size={12} />
            <span>Add to chat</span>
          </button>
          <button
            type="button"
            className="pc-act"
            aria-label="View comment on GitHub"
            onClick={() => openExternal(c.url)}
          >
            <Icon name="external" size={11} />
            <span>GitHub</span>
          </button>
          <div className="pc-acts-end">
            {!expanded && imageCount > 0 && (
              <span className="pc-media">
                <Icon name="image" size={11} />
                {imageCount} {imageCount === 1 ? "image" : "images"}
              </span>
            )}
            {expandable && (
              <button
                type="button"
                className="pc-act"
                aria-expanded={expanded}
                onClick={() => setExpanded((v) => !v)}
              >
                <span>{expanded ? "Show less" : "Show more"}</span>
                <Icon name={expanded ? "chevU" : "chevD"} size={11} />
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
