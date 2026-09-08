const COMMANDS: &[&str] = &["request_permission", "register", "unregister"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).ios_path("ios").build();

    // Remote notifications need the `aps-environment` entitlement, and Tauri's
    // `bundle.iOS` config has no entitlements key — so patch the generated
    // Xcode project the way tauri-plugin-deep-link patches it for associated
    // domains. It is a no-op unless the iOS CLI is driving the build (it keys
    // off TAURI_IOS_PROJECT_PATH), so a plain `cargo check` never touches a
    // file. `development` is what Xcode's own "Push Notifications" capability
    // writes, and Xcode replaces it when it signs with a distribution profile;
    // the phone reports the environment the embedded profile actually names, so
    // the relay routes to the right APNs host either way.
    #[cfg(target_os = "macos")]
    tauri_plugin::mobile::update_entitlements(|entitlements| {
        entitlements.insert("aps-environment".into(), "development".into());
    })
    .expect("failed to update the iOS entitlements");
}
