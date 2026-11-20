import type { ReactNode } from "react";
import type { DiffBaseMode } from "@/api";
import { useCheckoutFile } from "@/components/RightPanel/FilePanel/useCheckoutFile";
import { ImagePreview } from "@/components/RightPanel/ImagePreview";
import { basename } from "@/util/format";

/** What a changed file with no line-level diff shows: the image, when the file
 *  is one (a deleted image as it was at `base`), else `children`. */
export function ChangedImage({
  agentId,
  path,
  base,
  rev,
  children,
}: {
  agentId: string;
  path: string;
  base: DiffBaseMode;
  /** The file's revision; the image is read again when it moves. */
  rev: string;
  children: ReactNode;
}) {
  const { contents } = useCheckoutFile(agentId, path, base, rev);
  if (!contents?.image) return children;
  return <ImagePreview image={contents.image} name={basename(path)} />;
}
