import { readFile, stat } from "@tauri-apps/plugin-fs";
import { api } from "@/api";
import { activeEnvironment } from "@/store/environments";
import { assertWithinUploadCap } from "@/util/attachmentUpload";
import { baseName } from "@/util/paths";

/** What the composer should hold for a file on this Mac the user dropped or
 *  browsed to: a staged copy, never the path itself.
 *
 *  On this Mac the app copies it into its attachments staging area, from where
 *  it is adopted into the agent's workspace at send time. The path itself used
 *  to do, since agents could read anything the user can — but the sandbox now
 *  denies the folders macOS guards with a privacy prompt, and Downloads, where
 *  screenshots land, is the first of them. On a host the path names a file on
 *  this machine that the host cannot open at all, so the bytes go up and the
 *  composer holds the staged host path instead.
 *
 *  The cap is checked against the file's size before it is read for a host, so
 *  an archive dropped by mistake costs an error rather than hundreds of
 *  megabytes through the webview. The local copy is the app's own `fs::copy`
 *  and carries no cap.
 *
 *  The dialog plugin grants the fs scope for paths IT returned (see
 *  `util/image.ts`), so a picked path can be read here; a dropped one has no
 *  grant of its own, and the Rust `DragDrop` window-event handler in
 *  `src-tauri/src/lib.rs` calls the same `allow_file` for it. Anything else —
 *  a path this webview made up — is still refused, which is the point of not
 *  widening the capability instead. */
export async function stageAttachmentPath(path: string): Promise<string> {
  if (activeEnvironment().kind === "local") return api.stageAttachmentPath(path);
  const name = baseName(path);
  assertWithinUploadCap({ name, size: (await stat(path)).size });
  return api.savePastedAttachment(name, await readFile(path));
}

/** What the composer should hold for a file the user named by path on the
 *  environment being driven — an `@~/Downloads/shot.png` mention. Unlike a
 *  drop, the path is already the driven environment's own, so a host's path
 *  stands as it is; on this Mac it is staged like a drop, for the same
 *  reason. A checkout-relative mention (`@src/app.ts`) never comes here: it
 *  names a file the agent reads in place. */
export function stageEnvironmentPath(path: string): Promise<string> {
  if (activeEnvironment().kind === "local") return api.stageAttachmentPath(path);
  return Promise.resolve(path);
}
