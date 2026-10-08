// The iPhone side of discovery (docs/remote-protocol.md, "Discovery"): browse
// `_fletch._tcp` for a moment and hand the webview what each host's TXT record
// says about it. Nothing here is trusted — the Noise handshake that follows is
// what authenticates — so a record missing a field is skipped, not repaired.

import Foundation
import Network
import Tauri
import WebKit

struct BrowseArgs: Decodable {
  let durationMs: Int?
}

struct NearbyHost: Encodable {
  let name: String
  let hostKey: String
  let port: Int
}

struct BrowseResult: Encodable {
  let hosts: [NearbyHost]
}

class NearbyPlugin: Plugin {
  /// One browse per call, answered when `durationMs` is up. A denied local
  /// network permission leaves the browser waiting rather than failing, so the
  /// deadline is what ends every browse, with whatever has been seen by then.
  @objc public func browse(_ invoke: Invoke) {
    let args = try? invoke.parseArgs(BrowseArgs.self)
    let seconds = Double(max(args?.durationMs ?? 2000, 200)) / 1000
    let queue = DispatchQueue(label: "fletch.nearby")
    let browser = NWBrowser(
      for: .bonjourWithTXTRecord(type: "_fletch._tcp", domain: nil), using: NWParameters())
    // Touched only on `queue`, which every handler below runs on.
    var hosts: [String: NearbyHost] = [:]
    var answered = false
    let answer = {
      if answered { return }
      answered = true
      browser.cancel()
      invoke.resolve(BrowseResult(hosts: hosts.values.sorted { $0.name < $1.name }))
    }
    browser.browseResultsChangedHandler = { results, _ in
      var seen: [String: NearbyHost] = [:]
      for result in results {
        guard case let .bonjour(txt) = result.metadata,
          let name = txt["name"], let hostKey = txt["id"],
          let portText = txt["port"], let port = Int(portText)
        else { continue }
        seen[hostKey] = NearbyHost(name: name, hostKey: hostKey, port: port)
      }
      hosts = seen
    }
    browser.stateUpdateHandler = { state in
      if case .failed = state { answer() }
    }
    browser.start(queue: queue)
    queue.asyncAfter(deadline: .now() + seconds) { answer() }
  }
}

@_cdecl("init_plugin_nearby")
func initPlugin() -> Plugin {
  return NearbyPlugin()
}
