import { describe, expect, it, vi } from "vitest";
import {
  fitWithin,
  jpegName,
  MAX_UPLOAD_BYTES,
  prepareFile,
  shouldReencode,
} from "../src/attachments/prepare";
import {
  type AttachmentApi,
  CHUNK_BYTES,
  UploadAborted,
  uploadAttachment,
} from "../src/attachments/upload";
import { applyUserTurns } from "../src/store/transcript";

/** A host that records every op and reassembles what it was sent. */
function harness(opts: { failChunkAt?: number } = {}) {
  const ops: string[] = [];
  const received: Uint8Array[] = [];
  let chunks = 0;
  const api: AttachmentApi = {
    async attachmentBegin(name) {
      ops.push(`begin:${name}`);
      return { upload: "u1" };
    },
    async attachmentChunk(upload, data) {
      chunks += 1;
      ops.push(`chunk:${upload}`);
      if (opts.failChunkAt === chunks) throw new Error("Frame too large");
      received.push(Uint8Array.from(atob(data), (c) => c.charCodeAt(0)));
      return null;
    },
    async attachmentEnd(upload) {
      ops.push(`end:${upload}`);
      return { path: "/staged/u1/shot.png" };
    },
    async attachmentCancel(upload) {
      ops.push(`cancel:${upload}`);
      return null;
    },
  };
  const bytes = () => {
    const n = received.reduce((sum, c) => sum + c.length, 0);
    const out = new Uint8Array(n);
    let at = 0;
    for (const c of received) {
      out.set(c, at);
      at += c.length;
    }
    return out;
  };
  return { ops, api, bytes };
}

const pattern = (n: number) => Uint8Array.from({ length: n }, (_, i) => (i * 31) % 256);

describe("uploadAttachment", () => {
  it("slices under the frame cap, in order, and answers with the staged path", async () => {
    const h = harness();
    const file = pattern(CHUNK_BYTES * 2 + 1234);
    const path = await uploadAttachment(h.api, "shot.png", file);
    expect(path).toBe("/staged/u1/shot.png");
    expect(h.ops).toEqual(["begin:shot.png", "chunk:u1", "chunk:u1", "chunk:u1", "end:u1"]);
    expect(h.bytes()).toEqual(file);
  });

  it("an empty file is an upload with no chunks", async () => {
    const h = harness();
    await uploadAttachment(h.api, "empty.txt", new Uint8Array(0));
    expect(h.ops).toEqual(["begin:empty.txt", "end:u1"]);
  });

  it("a chunk that fails cancels the upload on the host and rejects", async () => {
    const h = harness({ failChunkAt: 2 });
    await expect(uploadAttachment(h.api, "big.png", pattern(CHUNK_BYTES * 3))).rejects.toThrow(
      "Frame too large",
    );
    // No chunk went out past the failure, and nothing was ended.
    expect(h.ops).toEqual(["begin:big.png", "chunk:u1", "chunk:u1", "cancel:u1"]);
  });

  it("an abort between chunks cancels rather than finishing for nobody", async () => {
    const h = harness();
    const controller = new AbortController();
    // Abort as soon as the first chunk is on the wire.
    const chunk = h.api.attachmentChunk.bind(h.api);
    h.api.attachmentChunk = async (u, d) => {
      controller.abort();
      return chunk(u, d);
    };
    await expect(
      uploadAttachment(h.api, "shot.png", pattern(CHUNK_BYTES * 2), controller.signal),
    ).rejects.toBeInstanceOf(UploadAborted);
    expect(h.ops).toEqual(["begin:shot.png", "chunk:u1", "cancel:u1"]);
  });

  it("an upload aborted before it starts never opens one", async () => {
    const h = harness();
    const controller = new AbortController();
    controller.abort();
    await expect(
      uploadAttachment(h.api, "shot.png", pattern(10), controller.signal),
    ).rejects.toBeInstanceOf(UploadAborted);
    expect(h.ops).toEqual([]);
  });
});

describe("prepareFile decisions", () => {
  it("re-encodes HEIC always, large rasters, and leaves the rest alone", () => {
    const f = (name: string, type: string, size: number) => ({ name, type, size });
    expect(shouldReencode(f("IMG_1.HEIC", "image/heic", 100))).toBe(true);
    expect(shouldReencode(f("IMG_1.heif", "", 100))).toBe(true);
    expect(shouldReencode(f("shot.png", "image/png", 300 * 1024))).toBe(false);
    expect(shouldReencode(f("shot.png", "image/png", 4 * 1024 * 1024))).toBe(true);
    expect(shouldReencode(f("anim.gif", "image/gif", 9 * 1024 * 1024))).toBe(false);
    expect(shouldReencode(f("logo.svg", "image/svg+xml", 9 * 1024 * 1024))).toBe(false);
    expect(shouldReencode(f("notes.pdf", "application/pdf", 9 * 1024 * 1024))).toBe(false);
  });

  it("names the re-encoded copy after the original stem", () => {
    expect(jpegName("IMG_0001.HEIC")).toBe("IMG_0001.jpg");
    expect(jpegName("Screenshot 2026-09-16 at 10.12.03.png")).toBe(
      "Screenshot 2026-09-16 at 10.12.03.jpg",
    );
    expect(jpegName("noext")).toBe("noext.jpg");
    expect(jpegName(".heic")).toBe("photo.jpg");
  });

  /** A picked file, with the read instrumented: the whole point of the cap is
   *  that an oversized file is refused before `arrayBuffer` is ever called. */
  function picked(name: string, type: string, size: number) {
    const arrayBuffer = vi.fn(async () => new ArrayBuffer(Math.min(size, 16)));
    return { file: { name, type, size, arrayBuffer } as unknown as File, arrayBuffer };
  }

  it("refuses a file over the host's cap before reading a byte of it", async () => {
    for (const [name, type] of [
      ["clip.mov", "video/quicktime"],
      ["repo.zip", "application/zip"],
      ["spec.pdf", "application/pdf"],
    ]) {
      const { file, arrayBuffer } = picked(name, type, 300 * 1024 * 1024);
      await expect(prepareFile(file)).rejects.toThrow(`${name} is 300 MB — the limit is 32 MB`);
      expect(arrayBuffer).not.toHaveBeenCalled();
    }
  });

  it("reads a file at the cap, and any smaller one, as it is", async () => {
    const { file, arrayBuffer } = picked("spec.pdf", "application/pdf", MAX_UPLOAD_BYTES);
    const out = await prepareFile(file);
    expect(out.name).toBe("spec.pdf");
    expect(arrayBuffer).toHaveBeenCalledTimes(1);
  });

  it("fits inside the longest edge without scaling up", () => {
    expect(fitWithin(4032, 3024)).toEqual({ w: 2048, h: 1536 });
    expect(fitWithin(1179, 2556)).toEqual({ w: 945, h: 2048 });
    expect(fitWithin(800, 600)).toEqual({ w: 800, h: 600 });
  });
});

describe("applyUserTurns with attachments", () => {
  it("hangs the turn's attachments on the bubble and restores the typed text", () => {
    const items = applyUserTurns(
      [
        { kind: "user_message", text: "older, no row" },
        {
          kind: "user_message",
          text: "what is wrong here?\nAttached file: /ws/.fletch-attachments/u/shot.png",
        },
        { kind: "agent_message", text: "Looking." },
      ],
      [
        {
          turn_id: "t1",
          seq: 1,
          text: "what is wrong here?",
          attachments: ["/ws/.fletch-attachments/u/shot.png"],
          native_id: "n1",
          started_at: 10,
          ended_at: 20,
        },
      ],
    );
    expect(items[0]).toEqual({ kind: "user_message", text: "older, no row" });
    expect(items[1]).toEqual({
      kind: "user_message",
      text: "what is wrong here?",
      attachments: ["/ws/.fletch-attachments/u/shot.png"],
      startedAt: 10,
      endedAt: 20,
    });
  });

  it("leaves the text alone when the row does not prefix it", () => {
    const [item] = applyUserTurns(
      [{ kind: "user_message", text: "something else entirely" }],
      [
        {
          turn_id: "t1",
          seq: 1,
          text: "typed",
          attachments: ["/a.png"],
          native_id: "n1",
          started_at: null,
          ended_at: null,
        },
      ],
    );
    expect(item).toEqual({
      kind: "user_message",
      text: "something else entirely",
      attachments: ["/a.png"],
    });
  });
});
