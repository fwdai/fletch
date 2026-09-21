import { readFile, stat } from "@tauri-apps/plugin-fs";
import { api } from "@/api";
import { activeEnvironment } from "@/store/environments";
import { assertWithinUploadCap } from "@/util/attachmentUpload";
import { baseName } from "@/util/paths";

/** What the composer should hold for a file the user dropped or browsed to.
 *
 *  On this Mac that is the path itself — the engine reading it is the one the
 *  picker browsed. On a host it is nothing at all: the path names a file on
 *  this machine, and `send_user_message` / `wf_launch` are routed there, so the
 *  bytes go up and the composer holds the staged host path instead.
 *
 *  The cap is checked against the file's size before it is read, so an archive
 *  dropped by mistake costs an error rather than hundreds of megabytes through
 *  the webview. */
export async function stageAttachmentPath(path: string): Promise<string> {
  if (activeEnvironment().kind === "local") return path;
  const name = baseName(path);
  assertWithinUploadCap({ name, size: (await stat(path)).size });
  return api.savePastedAttachment(name, await readFile(path));
}
