import CoreImage.CIFilterBuiltins
import SwiftUI
import ZuseCore

#if os(macOS)
  import AppKit
#else
  import UIKit
#endif

struct ComputerAccessView: View {
  let rpc: RPCClient
  @State private var devices: [PairedDevice] = []
  @State private var invitation: PairingInvitation?
  @State private var qrImage: NativeImage?
  @State private var busy = false
  @State private var active = false
  @State private var failure: String?
  @State private var revokeDevice: PairedDevice?
  @State private var requestID = UUID()
  var body: some View {
    List {
      if let failure { Text(failure).font(.caption).foregroundStyle(.red) }
      Section("Pair another device") {
        if let invitation {
          TimelineView(.periodic(from: .now, by: 1)) { context in
            if invitation.expiresAt > context.date {
              VStack(alignment: .leading, spacing: 8) {
                if let qrImage {
                  Image(nativeImage: qrImage).interpolation(.none).resizable()
                    .scaledToFit().frame(width: 168, height: 168)
                    .padding(12).background(.white, in: RoundedRectangle(cornerRadius: 8))
                    .accessibilityLabel("Pairing QR code")
                }
                LabeledContent("Code", value: invitation.code).font(
                  .system(.body, design: .monospaced)
                )
                .textSelection(.enabled).frame(height: 28)
                HStack {
                  Text("Expires").font(.caption).foregroundStyle(.secondary)
                  Text(invitation.expiresAt, style: .relative).font(.caption).foregroundStyle(
                    .secondary)
                  Spacer()
                  Button("Copy link") { NativePlatform.copy(invitation.browserURL.absoluteString) }
                    .frame(height: 28)
                  ShareLink(item: invitation.browserURL) {
                    Image(systemName: "square.and.arrow.up")
                  }
                  .accessibilityLabel("Share pairing link").frame(height: 28)
                }
              }
            } else {
              Text("This code expired. Generate a new one to pair another device.")
                .font(.caption).foregroundStyle(.secondary)
            }
          }
        }
        Button(
          invitation == nil ? "Generate pairing code" : "Generate a new code", systemImage: "qrcode"
        ) {
          Task { await generate() }
        }.disabled(busy).frame(height: 28)
        Text("Scan this code in Zuse on your other device, or share the pairing link privately.")
          .font(.caption).foregroundStyle(.secondary)
      }
      Section("Paired devices") {
        if busy { ProgressView("Working…") }
        ForEach(devices) { device in
          HStack {
            VStack(alignment: .leading, spacing: 3) {
              Text(device.displayName)
              if let value = device.lastUsedAt, let date = ServerTimestamp.parse(value) {
                Text("Last used \(date, style: .relative)").font(.caption).foregroundStyle(
                  .secondary)
              } else {
                Text("Not used yet").font(.caption).foregroundStyle(.secondary)
              }
            }
            Spacer()
            Button("Revoke", role: .destructive) { revokeDevice = device }
              .disabled(busy).frame(height: 28)
          }
        }
        if devices.isEmpty && !busy && failure == nil {
          Text("No paired devices").font(.caption).foregroundStyle(.secondary)
        }
        Button("Refresh devices") { Task { await load() } }.disabled(busy).frame(height: 28)
      }
    }.settingsList().navigationTitle("Computer access").compactNavigationTitle()
      .task {
        active = true
        await load()
      }
      .onDisappear {
        active = false
        requestID = UUID()
        busy = false
        invitation = nil
        qrImage = nil
        revokeDevice = nil
      }
      .confirmationDialog(
        "Revoke this device's access to this computer?",
        isPresented: Binding(get: { revokeDevice != nil }, set: { if !$0 { revokeDevice = nil } })
      ) {
        if let device = revokeDevice {
          Button("Revoke " + device.displayName, role: .destructive) {
            revokeDevice = nil
            Task { await revoke(device) }
          }
        }
      } message: {
        Text(
          "This device will need to pair again. Revoking this device may disconnect your current session."
        )
      }
  }
  private func load() async {
    guard active, !busy else { return }
    let request = UUID()
    requestID = request
    busy = true
    failure = nil
    defer { if requestID == request { busy = false } }
    do {
      let devices = try await rpc.call("pairing.listTokens").decode([PairedDevice].self)
      guard active, requestID == request, !Task.isCancelled else { return }
      self.devices = devices.filter { $0.revokedAt == nil }
    } catch is CancellationError {} catch {
      if requestID == request { failure = error.localizedDescription }
    }
  }
  private func generate() async {
    guard active, !busy else { return }
    let request = UUID()
    requestID = request
    busy = true
    failure = nil
    defer { if requestID == request { busy = false } }
    do {
      let result = try await rpc.call("pairing.start")
      guard active, requestID == request, !Task.isCancelled else { return }
      let invitation = try PairingInvitation(result)
      self.invitation = invitation
      let filter = CIFilter.qrCodeGenerator()
      filter.message = Data(invitation.browserURL.absoluteString.utf8)
      filter.correctionLevel = "M"
      if let output = filter.outputImage,
        let image = CIContext().createCGImage(output, from: output.extent)
      {
        #if os(macOS)
          qrImage = NSImage(cgImage: image, size: output.extent.size)
        #else
          qrImage = UIImage(cgImage: image)
        #endif
      }
    } catch is CancellationError {} catch {
      if requestID == request { failure = error.localizedDescription }
    }
  }
  private func revoke(_ device: PairedDevice) async {
    guard active, !busy else { return }
    let request = UUID()
    requestID = request
    busy = true
    failure = nil
    defer { if requestID == request { busy = false } }
    do {
      _ = try await rpc.call("pairing.revokeToken", ["tokenId": .string(device.id)])
      guard active, requestID == request, !Task.isCancelled else { return }
      devices.removeAll { $0.id == device.id }
    } catch is CancellationError {} catch {
      if requestID == request { failure = error.localizedDescription }
    }
  }
}
