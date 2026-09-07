// The mobile shell registers plugins and the secure channel to the paired Mac.
// Code and agents run on the host; the only Rust the phone needs of its own is
// `remote`, because the Noise handshake and the socket cannot live in a webview.

mod remote;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_deep_link::init())
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
