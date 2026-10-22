import { open } from "@tauri-apps/plugin-shell";
import { type ImgHTMLAttributes, useEffect, useMemo, useState } from "react";
import type { Components } from "react-markdown";
import type { PrComment } from "@/api";
import { Icon } from "@/components/Icon";
import { Markdown } from "@/components/Markdown";
import { ProviderIcon } from "@/components/ProviderIcon";
import { commentLocation } from "@/components/RightPanel/prComments";
import { providerChip } from "@/data/providers";
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

/** Who left the thread, at a glance: the local reviewer's provider mark, a
 *  bot, or a person. */
function SourceMark({
  comment: c,
  localProvider,
}: {
  comment: PrComment;
  localProvider: string | null;
}) {
  if (c.source === "local" && localProvider) {
    const { short, hue } = providerChip(localProvider);
    return <ProviderIcon slug={localProvider} short={short} hue={hue} size={14} />;
  }
  return <Icon name={c.is_bot ? "bot" : "user"} size={12} />;
}

export function CommentRow({
  comment: c,
  onAddToChat,
  onFix,
  onDismiss,
  onOpenFile,
  localProvider,
}: {
  comment: PrComment;
  onAddToChat: (c: PrComment) => void;
  onFix?: (c: PrComment) => void;
  onDismiss?: (c: PrComment) => void;
  /** Present only when the file can be opened (it is among the changed files). */
  onOpenFile?: (path: string) => void;
  localProvider: string | null;
}) {
  const [expanded, setExpanded] = useState(false);
  // Dismissing closes a finding for good, so it takes two clicks: the first
  // arms the button for a moment, the second fires.
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    if (!armed) return;
    const t = setTimeout(() => setArmed(false), 3000);
    return () => clearTimeout(t);
  }, [armed]);
  // GitHub bodies mix markdown with HTML (pasted screenshots, bot wrappers).
  // Normalise to markdown once; the preview flattens that to prose.
  const md = useMemo(() => htmlToMarkdown(c.body), [c.body]);
  const text = useMemo(() => markdownToText(md), [md]);
  const imageCount = useMemo(() => markdownImages(md).length, [md]);
  const loc = commentLocation(c);
  // Anything the one-line preview can't show faithfully is worth expanding:
  // long prose, images, or a code block (a bot's suggested fix, typically).
  const expandable = text.length > PREVIEW_CHARS || imageCount > 0 || /```|~~~/.test(md);
  const local = c.source === "local";

  return (
    <div className={expanded ? "pr-comment is-open" : "pr-comment"}>
      <SourceMark comment={c} localProvider={localProvider} />
      <div className="pc-body">
        <div className="pc-top text-xs">
          <span className="pc-author">{c.author}</span>
          {c.severity && <span className={`pc-sev s-${c.severity}`}>{c.severity}</span>}
          {loc &&
            (onOpenFile && c.path ? (
              <button
                type="button"
                className="pc-loc pc-loc-link text-xs"
                title="Open the file's diff"
                onClick={() => onOpenFile(c.path as string)}
              >
                {loc}
              </button>
            ) : (
              <span className="pc-loc text-xs">{loc}</span>
            ))}
          {c.we_replied_last ? (
            <span className="pc-replies" title="The agent answered and left it open for you">
              replied
            </span>
          ) : (
            c.replies > 0 && (
              <span className="pc-replies">
                +{c.replies} {c.replies === 1 ? "reply" : "replies"}
              </span>
            )
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
          {onFix && (
            <button
              type="button"
              className="pc-act pc-act--primary"
              aria-label="Fix with agent"
              onClick={() => onFix(c)}
            >
              <Icon name="wrench" size={12} />
              <span>Fix with agent</span>
            </button>
          )}
          <button
            type="button"
            className={onFix ? "pc-act" : "pc-act pc-act--primary"}
            aria-label="Add comment to chat"
            onClick={() => onAddToChat(c)}
          >
            <Icon name="chatPlus" size={12} />
            <span>Add to chat</span>
          </button>
          {c.url && (
            <button
              type="button"
              className="pc-act"
              aria-label="View comment on GitHub"
              onClick={() => openExternal(c.url as string)}
            >
              <Icon name="external" size={11} />
              <span>GitHub</span>
            </button>
          )}
          {local && onDismiss && (
            <button
              type="button"
              className={armed ? "pc-act pc-act--danger" : "pc-act"}
              aria-label={armed ? "Confirm dismiss" : "Dismiss finding"}
              onClick={() => {
                if (armed) {
                  setArmed(false);
                  onDismiss(c);
                } else {
                  setArmed(true);
                }
              }}
            >
              <Icon name={armed ? "check" : "close"} size={11} />
              <span>{armed ? "Dismiss? Click again" : "Dismiss"}</span>
            </button>
          )}
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
