import { open as openExternal } from "@tauri-apps/plugin-shell";
import type { ILinkHandler } from "@xterm/xterm";

/** OSC 8 hyperlinks; xterm's default runs `confirm()`, which the Tauri webview can't. */
export const hyperlinkHandler: ILinkHandler = {
  activate: (_event, uri) => {
    openExternal(uri).catch((err) => {
      console.error("open link failed", err);
    });
  },
};
