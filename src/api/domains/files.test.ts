// Where a pasted or dropped file is staged. The message that carries the path
// (`send_user_message`, `wf_launch`) goes to the engine the api drives, so
// staging has to follow it: a path written into this Mac's app-data folder is
// a path a host cannot open, and the attachment silently does nothing there.

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Transport } from "@/api/transport";
import { CHUNK_BYTES, MAX_UPLOAD_BYTES } from "@/util/attachmentUpload";
import { invokeLocal } from "../invoke";
import { filesApi } from "./files";

const { rawInvoke } = vi.hoisted(() => ({ rawInvoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: rawInvoke }));

/** Records every op the host is asked for, and answers the upload flow the way
 *  a host does. */
function fakeTransport(after?: (op: string) => void) {
  const calls: { op: string; args?: Record<string, unknown> }[] = [];
  const transport: Transport = {
    call: <T>(op: string, args?: Record<string, unknown>): Promise<T> => {
      calls.push({ op, args });
      after?.(op);
      if (op === "attachment_begin") return Promise.resolve({ upload: "u1" } as T);
      if (op === "attachment_end") {
        return Promise.resolve({ path: "/host/.fletch/staging/u1/shot.png" } as T);
      }
      return Promise.resolve(null as T);
    },
    on: () => Promise.resolve(() => {}),
  };
  return { calls, transport };
}

/** The files api of a paired host whose connection is whatever `current` says
 *  when it is asked. */
const hostFiles = (current: () => Transport) =>
  filesApi((op, args) => current().call(op, args), current);

describe("savePastedAttachment", () => {
  beforeEach(() => {
    rawInvoke.mockReset();
    rawInvoke.mockResolvedValue("/Users/alex/Library/…/shot.png");
  });

  it("writes to this Mac's staging area for the local engine", async () => {
    const path = await filesApi(invokeLocal).savePastedAttachment("shot 1.png", new Uint8Array(4));

    expect(path).toBe("/Users/alex/Library/…/shot.png");
    // Name reduced to a plain ASCII basename: it travels as a header.
    expect(rawInvoke).toHaveBeenCalledWith("save_pasted_attachment", expect.any(Uint8Array), {
      headers: { name: "shot_1.png" },
    });
  });

  it("uploads to the host and answers with the path the host staged", async () => {
    const { calls, transport } = fakeTransport();

    // Two chunks and a remainder, so the slicing is exercised rather than
    // assumed.
    const bytes = new Uint8Array(CHUNK_BYTES * 2 + 5);
    const path = await hostFiles(() => transport).savePastedAttachment("shot.png", bytes);

    expect(path).toBe("/host/.fletch/staging/u1/shot.png");
    expect(calls.map((c) => c.op)).toEqual([
      "attachment_begin",
      "attachment_chunk",
      "attachment_chunk",
      "attachment_chunk",
      "attachment_end",
    ]);
    expect(calls[0].args).toEqual({ name: "shot.png" });
    // Nothing touched this Mac's staging area.
    expect(rawInvoke).not.toHaveBeenCalled();
  });

  it("finishes an upload on the connection it started on", async () => {
    // The host's connection is replaced while the chunks are going up.
    // `attachment_begin` answered with an id that exists only on the first,
    // so every later op has to go back there: sent down the second, they name
    // nothing — and the first is left holding a partial file nothing will
    // ever cancel.
    const second = fakeTransport();
    let current: Transport;
    const first = fakeTransport((op) => {
      if (op === "attachment_begin") current = second.transport;
    });
    current = first.transport;

    const bytes = new Uint8Array(CHUNK_BYTES + 5);
    const path = await hostFiles(() => current).savePastedAttachment("shot.png", bytes);

    expect(path).toBe("/host/.fletch/staging/u1/shot.png");
    expect(first.calls.map((c) => c.op)).toEqual([
      "attachment_begin",
      "attachment_chunk",
      "attachment_chunk",
      "attachment_end",
    ]);
    expect(second.calls, "the later connection was never asked about this upload").toEqual([]);
  });

  it("cancels on the connection it started on when a chunk fails", async () => {
    // The same binding on the failure path: the partial file is behind the
    // first connection, so that is the only one that can drop it.
    const second = fakeTransport();
    let current: Transport;
    const first = {
      calls: [] as { op: string }[],
      transport: {
        call: <T>(op: string): Promise<T> => {
          first.calls.push({ op });
          if (op === "attachment_begin") return Promise.resolve({ upload: "u1" } as T);
          if (op === "attachment_chunk") {
            current = second.transport;
            return Promise.reject(new Error("socket closed"));
          }
          return Promise.resolve(null as T);
        },
        on: () => Promise.resolve(() => {}),
      } as Transport,
    };
    current = first.transport;

    await expect(
      hostFiles(() => current).savePastedAttachment("shot.png", new Uint8Array(4)),
    ).rejects.toThrow(/socket closed/);

    expect(first.calls.map((c) => c.op)).toEqual([
      "attachment_begin",
      "attachment_chunk",
      "attachment_cancel",
    ]);
    expect(second.calls).toEqual([]);
  });

  it("refuses a file over the host's cap before uploading a byte of it", async () => {
    const { calls, transport } = fakeTransport();

    await expect(
      hostFiles(() => transport).savePastedAttachment(
        "clip.mov",
        new Uint8Array(MAX_UPLOAD_BYTES + 1),
      ),
    ).rejects.toThrow(/limit is 32 MB/);
    expect(calls).toEqual([]);
  });
});
