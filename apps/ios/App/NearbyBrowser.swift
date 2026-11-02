import Foundation
import SwiftUI

@MainActor @Observable
final class NearbyBrowser: NSObject, @preconcurrency NetServiceBrowserDelegate,
  @preconcurrency NetServiceDelegate
{
  struct Computer: Identifiable {
    let id: String
    let name: String
    let url: URL
    let pin: String
  }
  private var browser: NetServiceBrowser?
  private var services: [NetService] = []
  var computers: [Computer] = []
  var error: String?
  func start() {
    stop()
    error = nil
    let browser = NetServiceBrowser()
    self.browser = browser
    browser.delegate = self
    browser.includesPeerToPeer = true
    browser.searchForServices(ofType: "_zuse._tcp.", inDomain: "local.")
  }
  func stop() {
    browser?.delegate = nil
    browser?.stop()
    browser = nil
    for service in services {
      service.delegate = nil
      service.stop()
    }
    services = []
    computers = []
  }
  func netServiceBrowser(
    _ browser: NetServiceBrowser, didFind service: NetService, moreComing: Bool
  ) {
    guard browser === self.browser else { return }
    services.append(service)
    service.delegate = self
    service.includesPeerToPeer = true
    service.resolve(withTimeout: 10)
  }
  func netServiceBrowser(
    _ browser: NetServiceBrowser, didRemove service: NetService, moreComing: Bool
  ) {
    guard browser === self.browser else { return }
    services.removeAll { $0 == service }
    computers.removeAll { $0.id == service.name + service.domain }
  }
  func netServiceBrowser(_ browser: NetServiceBrowser, didNotSearch errorDict: [String: NSNumber]) {
    guard browser === self.browser else { return }
    error =
      "Nearby discovery is unavailable. Check Local Network access in Settings, or paste a pairing link."
  }
  func netServiceDidResolveAddress(_ service: NetService) {
    guard services.contains(where: { $0 === service }) else { return }
    guard let host = service.hostName, let txt = service.txtRecordData(),
      let pinBytes = NetService.dictionary(fromTXTRecord: txt)["tls"],
      let pin = String(data: pinBytes, encoding: .utf8), pin.count == 43
    else { return }
    var url = URLComponents()
    url.scheme = "https"
    url.host = host
    url.port = service.port
    guard let address = url.url else { return }
    let computer = Computer(
      id: service.name + service.domain, name: service.name, url: address, pin: pin)
    computers.removeAll { $0.id == computer.id }
    computers.append(computer)
  }
}
