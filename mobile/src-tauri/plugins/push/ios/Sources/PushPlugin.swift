// The iPhone side of push (docs/remote-protocol.md, "Push notifications"): hand
// the webview the APNs device token and the `fletch` object of a tapped alert.
// Nothing else — the alert's text is composed on the Mac and delivered by the
// relay, and this file never sees it.

import Foundation
import SwiftRs
import Tauri
import UIKit
import UserNotifications
import WebKit

/// `push://token`: the token as lowercase hex, plus which APNs host will reach
/// this build. The host sends both to the relay verbatim.
struct TokenPayload: Codable {
  let token: String
  let environment: String
}

/// The `fletch` object the relay puts next to `aps`. Every field is optional
/// because it comes off the wire; the app checks `hostId` before it navigates.
struct FletchPayload: Codable {
  let hostId: String?
  let agentId: String?
  let kind: String?
}

/// `push://opened`.
struct OpenedPayload: Codable {
  let fletch: FletchPayload
}

class PushPlugin: Plugin, UNUserNotificationCenterDelegate {
  /// The app-delegate callbacks are installed on a class Tao owns, so their
  /// implementations have no instance to reach through — they come back here.
  /// There is exactly one plugin instance for the life of the process.
  static weak var current: PushPlugin?

  /// APNs hands over the token, and iOS reports a tapped alert, before the
  /// webview has a listener — on a cold start from the notification, long
  /// before. Both are held here and flushed by `register`, which the app calls
  /// only once its listeners are attached. Everything below touches these on
  /// the main queue and nowhere else, which is the whole of the locking.
  private var pendingToken: TokenPayload?
  private var pendingOpened: [OpenedPayload] = []
  private var flushing = false
  private var injected = false

  public override func load(webview: WKWebView) {
    PushPlugin.current = self
    // Only one object can hold this delegate, and tauri-plugin-notification
    // claims it too. This plugin is registered after it in lib.rs and
    // registration is what loads it, so this assignment is the last one and
    // wins. Nothing is lost today because the phone raises no local
    // notifications; if it ever does, their responses have to be forwarded on
    // from here.
    UNUserNotificationCenter.current().delegate = self
    installAppDelegateCallbacks()
  }

  // MARK: - Commands

  @objc public func requestPermission(_ invoke: Invoke) {
    UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) {
      granted, error in
      if let error = error {
        invoke.reject(error.localizedDescription)
        return
      }
      invoke.resolve(["permissionState": granted ? "granted" : "denied"])
    }
  }

  /// Ask APNs for a token, and open the gate on the buffer: from here on the
  /// webview is listening, so anything held (or arriving later) is sent.
  @objc public func register(_ invoke: Invoke) {
    DispatchQueue.main.async {
      self.flushing = true
      UIApplication.shared.registerForRemoteNotifications()
      self.flush()
    }
    invoke.resolve()
  }

  @objc public func unregister(_ invoke: Invoke) {
    DispatchQueue.main.async {
      UIApplication.shared.unregisterForRemoteNotifications()
    }
    invoke.resolve()
  }

  // MARK: - UNUserNotificationCenterDelegate

  /// Present nothing while the app is in the foreground: the user is already
  /// looking at Fletch, and the protocol leans on iOS doing exactly this.
  public func userNotificationCenter(
    _ center: UNUserNotificationCenter,
    willPresent notification: UNNotification,
    withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
  ) {
    completionHandler([])
  }

  public func userNotificationCenter(
    _ center: UNUserNotificationCenter,
    didReceive response: UNNotificationResponse,
    withCompletionHandler completionHandler: @escaping () -> Void
  ) {
    if let payload = PushPlugin.opened(from: response.notification.request.content.userInfo) {
      DispatchQueue.main.async {
        self.pendingOpened.append(payload)
        self.flush()
      }
    }
    completionHandler()
  }

  // MARK: - APNs

  fileprivate func didRegister(deviceToken: Data) {
    let payload = TokenPayload(
      token: deviceToken.map { String(format: "%02x", $0) }.joined(),
      environment: PushPlugin.apsEnvironment())
    DispatchQueue.main.async {
      self.pendingToken = payload
      self.flush()
    }
  }

  /// `gen/apple` is generated and gitignored, so there is no AppDelegate to
  /// edit, and Tauri's `Plugin` base class forwards no app-delegate callbacks
  /// (unlike the deep link, which the Tauri core itself turns into a
  /// `RunEvent`). The two APNs callbacks are therefore added to Tao's delegate
  /// class at runtime. Nothing in Tao implements them, so this adds a method
  /// rather than replacing one; `method_setImplementation` is only the fallback
  /// in case a future template does, and nothing is lost by winning that race —
  /// Tao does not act on APNs.
  ///
  /// This runs from `load`, which is where the app delegate is reachable: the
  /// plugin is constructed while the Rust side is still setting up, and only
  /// loaded once there is a webview — by which time the app has launched.
  private func installAppDelegateCallbacks() {
    guard !injected, let delegate = UIApplication.shared.delegate else { return }
    injected = true
    let cls: AnyClass = type(of: delegate)

    let onToken: @convention(block) (AnyObject, UIApplication, NSData) -> Void = { _, _, token in
      PushPlugin.current?.didRegister(deviceToken: token as Data)
    }
    install(
      imp_implementationWithBlock(onToken),
      "application:didRegisterForRemoteNotificationsWithDeviceToken:", on: cls)

    let onFailure: @convention(block) (AnyObject, UIApplication, NSError) -> Void = { _, _, error in
      Logger.error("APNs registration failed: \(error.localizedDescription)")
    }
    install(
      imp_implementationWithBlock(onFailure),
      "application:didFailToRegisterForRemoteNotificationsWithError:", on: cls)
  }

  /// `v@:@@` — returns void, takes the two object arguments both
  /// `application(_:…)` callbacks above have.
  private func install(_ imp: IMP, _ name: String, on cls: AnyClass) {
    let selector = Selector(name)
    if !class_addMethod(cls, selector, imp, "v@:@@"),
      let method = class_getInstanceMethod(cls, selector)
    {
      method_setImplementation(method, imp)
    }
  }

  // MARK: - Helpers

  private func flush() {
    guard flushing else { return }
    if let token = pendingToken {
      pendingToken = nil
      try? trigger("token", data: token)
    }
    let opened = pendingOpened
    pendingOpened = []
    for payload in opened {
      try? trigger("opened", data: payload)
    }
  }

  private static func opened(from userInfo: [AnyHashable: Any]) -> OpenedPayload? {
    guard let fletch = userInfo["fletch"] as? [String: Any] else { return nil }
    return OpenedPayload(
      fletch: FletchPayload(
        hostId: fletch["hostId"] as? String,
        agentId: fletch["agentId"] as? String,
        kind: fletch["kind"] as? String))
  }

  /// Which APNs host can reach this build, read from the profile the build was
  /// actually signed with rather than from a compile-time flag — a TestFlight
  /// build of the same source needs `production`. A simulator build has no
  /// embedded profile and cannot receive a real push anyway, so it reports
  /// `sandbox`.
  private static func apsEnvironment() -> String {
    guard let url = Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision"),
      let raw = try? Data(contentsOf: url),
      // The profile is CMS-signed with a plist in the middle; slice the plist
      // out rather than pull in a decoder for the envelope.
      let text = String(data: raw, encoding: .isoLatin1),
      let start = text.range(of: "<?xml"),
      let end = text.range(of: "</plist>"),
      let plistData = String(text[start.lowerBound..<end.upperBound]).data(using: .isoLatin1),
      let plist = try? PropertyListSerialization.propertyList(
        from: plistData, options: [], format: nil) as? [String: Any],
      let entitlements = plist["Entitlements"] as? [String: Any],
      let environment = entitlements["aps-environment"] as? String
    else { return "sandbox" }
    return environment == "production" ? "production" : "sandbox"
  }
}

@_cdecl("init_plugin_push")
func initPlugin() -> Plugin {
  return PushPlugin()
}
