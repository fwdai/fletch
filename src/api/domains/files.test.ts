// Where a pasted or dropped file is staged. The message that carries the path
// (`send_user_message`, `wf_launch`) goes to the environment being driven, so
// staging has to follow it: a path written into this Mac's app-data folder is
// a path a host cannot open, and the attachment silently does nothing there.

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Transport } from "@/api/transport";
import {
  type EnvironmentEntry,
  LOCAL_ENVIRONMENT_ID,
  setEnvironmentsSource,
} from "@/store/environments";
import { CHUNK_BYTES, MAX_UPLOAD_BYTES } from "@/util/attachmentUpload";
import { filesApi } from "./files";

const { rawInvoke } = vi.hoisted(() => ({ rawInvoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: rawInvoke }));

/** Records every op the active environment is asked for, and answers the
 *  upload flow the way a host does. */
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

const onLocal = () =>
  setEnvironmentsSource(() => ({
    activeEnvironmentId: LOCAL_ENVIRONMENT_ID,
    environments: {},
  }));

const onHost = (transport: Transport, id = "host-1") => {
  const entry: EnvironmentEntry = {
    id,
    name: "Cloud box",
    kind: "remote",
    connection: "connected",
    transport,
  };
  setEnvironmentsSource(() => ({
    activeEnvironmentId: entry.id,
    environments: { [entry.id]: entry },
  }));
};

describe("savePastedAttachment", () => {
  beforeEach(() => {
    rawInvoke.mockReset();
    rawInvoke.mockResolvedValue("/Users/alex/Library/…/shot.png");
  });

  it("writes to this Mac's staging area for the local environment", async () => {
    onLocal();

    const path = await filesApi.savePastedAttachment("shot 1.png", new Uint8Array(4));

    expect(path).toBe("/Users/alex/Library/…/shot.png");
    // Name reduced to a plain ASCII basename: it travels as a header.
    expect(rawInvoke).toHaveBeenCalledWith("save_pasted_attachment", expect.any(Uint8Array), {
      headers: { name: "shot_1.png" },
    });
  });

  it("uploads to the host and answers with the path the host staged", async () => {
    const { calls, transport } = fakeTransport();
    onHost(transport);

    // Two chunks and a remainder, so the slicing is exercised rather than
    // assumed.
    const bytes = new Uint8Array(CHUNK_BYTES * 2 + 5);
    const path = await filesApi.savePastedAttachment("shot.png", bytes);

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

  it("finishes an upload on the environment it started on", async () => {
    // The user switches hosts while the chunks are going up. `attachment_begin`
    // answered with an id that exists only on the first host, so every later op
    // has to go back there: sent to the second, they name nothing — and the
    // first is left holding a partial file nothing will ever cancel.
    const second = fakeTransport();
    const first = fakeTransport((op) => {
      if (op === "attachment_begin") onHost(second.transport, "host-2");
    });
    onHost(first.transport);

    const bytes = new Uint8Array(CHUNK_BYTES + 5);
    const path = await filesApi.savePastedAttachment("shot.png", bytes);

    expect(path).toBe("/host/.fletch/staging/u1/shot.png");
    expect(first.calls.map((c) => c.op)).toEqual([
      "attachment_begin",
      "attachment_chunk",
      "attachment_chunk",
      "attachment_end",
    ]);
    expect(second.calls, "the host switched to was never asked about this upload").toEqual([]);
  });

  it("cancels on the environment it started on when a chunk fails", async () => {
    // The same binding on the failure path: the partial file is on the first
    // host, so that is the only host that can drop it.
    const second = fakeTransport();
    const first = {
      calls: [] as { op: string }[],
      transport: {
        call: <T>(op: string): Promise<T> => {
          first.calls.push({ op });
          if (op === "attachment_begin") return Promise.resolve({ upload: "u1" } as T);
          if (op === "attachment_chunk") {
            onHost(second.transport, "host-2");
            return Promise.reject(new Error("socket closed"));
          }
          return Promise.resolve(null as T);
        },
        on: () => Promise.resolve(() => {}),
      } as Transport,
    };
    onHost(first.transport);

    await expect(filesApi.savePastedAttachment("shot.png", new Uint8Array(4))).rejects.toThrow(
      /socket closed/,
    );

    expect(first.calls.map((c) => c.op)).toEqual([
      "attachment_begin",
      "attachment_chunk",
      "attachment_cancel",
    ]);
    expect(second.calls).toEqual([]);
  });

  it("refuses a file over the host's cap before uploading a byte of it", async () => {
    const { calls, transport } = fakeTransport();
    onHost(transport);

    await expect(
      filesApi.savePastedAttachment("clip.mov", new Uint8Array(MAX_UPLOAD_BYTES + 1)),
    ).rejects.toThrow(/limit is 32 MB/);
    expect(calls).toEqual([]);
  });
});
