/** Open a URL outside the app, the way every link in the phone's markup does:
 *  an anchor with `target="_blank"`, which the webview hands to the system
 *  browser. One helper so a button can do what an `<a>` does without each
 *  call site guessing at `window.open`'s behaviour in a WKWebView. */
export function openExternal(url: string): void {
  const a = document.createElement("a");
  a.href = url;
  a.target = "_blank";
  a.rel = "noreferrer";
  a.click();
}
