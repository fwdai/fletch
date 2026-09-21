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
function fakeTransport() {
  const calls: { op: string; args?: Record<string, unknown> }[] = [];
  const transport: Transport = {
    call: <T>(op: string, args?: Record<string, unknown>): Promise<T> => {
      calls.push({ op, args });
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

const onHost = (transport: Transport) => {
  const entry: EnvironmentEntry = {
    id: "host-1",
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

  it("refuses a file over the host's cap before uploading a byte of it", async () => {
    const { calls, transport } = fakeTransport();
    onHost(transport);

    await expect(
      filesApi.savePastedAttachment("clip.mov", new Uint8Array(MAX_UPLOAD_BYTES + 1)),
    ).rejects.toThrow(/limit is 32 MB/);
    expect(calls).toEqual([]);
  });
});
