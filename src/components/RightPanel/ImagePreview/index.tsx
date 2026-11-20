// A checkout image, scaled down to fit its pane and drawn over a checkerboard
// so transparency reads as such. Always through `<img>`, where an SVG's
// scripts do not run.
import { useState } from "react";
import type { CheckoutImage } from "@/api";

/** "image/svg+xml" → "SVG". */
const formatLabel = (mime: string) => mime.replace("image/", "").split("+")[0].toUpperCase();

export function ImagePreview({ image, name }: { image: CheckoutImage; name: string }) {
  const src = `data:${image.mime};base64,${image.data}`;
  // Both are remembered with the `src` they describe, so a file that changes
  // under the preview starts over.
  const [failed, setFailed] = useState<string | null>(null);
  const [size, setSize] = useState<{ src: string; width: number; height: number } | null>(null);

  if (failed === src) {
    return (
      <div className="empty-msg" style={{ margin: "auto" }}>
        <div className="et">No preview</div>
        <div>This image can't be shown here.</div>
      </div>
    );
  }

  // An SVG with no size of its own reports none.
  const measured = size?.src === src && size.width > 0 && size.height > 0 ? size : null;
  return (
    <div className="img-preview">
      <div className="img-preview-stage">
        <img
          src={src}
          alt={name}
          draggable={false}
          onLoad={(e) =>
            setSize({
              src,
              width: e.currentTarget.naturalWidth,
              height: e.currentTarget.naturalHeight,
            })
          }
          onError={() => setFailed(src)}
        />
      </div>
      <div className="img-preview-meta text-xs">
        {formatLabel(image.mime)}
        {measured && ` · ${measured.width} × ${measured.height}`}
      </div>
    </div>
  );
}
