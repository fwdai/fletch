// The mobile shell registers plugins and the secure channel to the paired Mac.
// Code and agents run on the host; the only Rust the phone needs of its own is
// `remote`, because the Noise handshake and the socket cannot live in a webview.

mod remote;

/// Hand the safe-area insets back to CSS, so the web content owns the whole
/// screen.
///
/// wry builds the `WKWebView` at the full size of the window, but never touches
/// its scroll view's `contentInsetAdjustmentBehavior` — which defaults to
/// `.automatic`. UIKit then insets that scroll view by the safe area, and WebKit
/// measures the layout viewport against the *un-inset* region, so a
/// `position: fixed; inset: 0` shell (`.m-app`) comes out short by the status bar
/// plus the home-indicator strip. The bottom-anchored composer ends up floating
/// well above the bottom of the screen with window background beneath it.
///
/// `.never` leaves the viewport at full screen size, which is what
/// `index.html`'s `viewport-fit=cover` and the `--safe-top` / `--safe-bottom`
/// custom properties are already written against.
#[cfg(target_os = "ios")]
fn own_the_whole_screen(webview: tauri::webview::PlatformWebview) {
    /// `UIScrollViewContentInsetAdjustmentNever`.
    const NEVER: isize = 2;

    let wk = webview.inner() as *mut objc2::runtime::AnyObject;
    // SAFETY: `inner()` is the window's live WKWebView, and Tauri runs this
    // closure on the main thread — where UIKit requires both calls to happen.
    unsafe {
        let scroll: *mut objc2::runtime::AnyObject = objc2::msg_send![wk, scrollView];
        let _: () = objc2::msg_send![scroll, setContentInsetAdjustmentBehavior: NEVER];
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_push::init())
        .setup(|_app| {
            #[cfg(target_os = "ios")]
            {
                use tauri::Manager;
                for webview in _app.webview_windows().values() {
                    webview.with_webview(own_the_whole_screen)?;
                }
            }
            Ok(())
        })
        .manage(remote::Remote::default())
        .invoke_handler(tauri::generate_handler![
            remote::remote_connect,
            remote::remote_send,
            remote::remote_close,
            remote::remote_device_public_key,
        ])
        .run(tauri::generate_context!())
        .expect("error while running fletch mobile");
}
