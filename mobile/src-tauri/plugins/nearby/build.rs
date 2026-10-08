const COMMANDS: &[&str] = &["browse"];

fn main() {
    // Browsing needs `NSBonjourServices` and `NSLocalNetworkUsageDescription`
    // in the app's Info.plist, which `mobile/src-tauri/Info.ios.plist` carries.
    // No entitlement: NWBrowser goes through mDNSResponder, so the multicast
    // entitlement a raw-socket browser would need does not apply.
    tauri_plugin::Builder::new(COMMANDS).ios_path("ios").build();
}
