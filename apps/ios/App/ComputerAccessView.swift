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
                HStack {
                  Text("Expires").font(.caption).foregroundStyle(.secondary)
                  Text(invitation.expiresAt, style: .relative).font(.caption).foregroundStyle(
                    .secondary)
                  Spacer()
                  Button("Copy link") { NativePlatform.copy(invitation.link.absoluteString) }
                    .frame(height: 28)
                  ShareLink(item: invitation.link) {
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
        Text(
          "Scan this code with Zuse on your other device, or share the link privately. It works once and expires in 10 minutes."
        )
        .font(.caption).foregroundStyle(.secondary)
      }
      Section("Paired devices") {
        if busy { ProgressView("Working…") }
        ForEach(devices) { device in
          HStack {
            VStack(alignment: .leading, spacing: 3) {
              Text(device.displayName)
              if let date = device.pairedDate {
                Text("Paired \(date, style: .relative) ago").font(.caption).foregroundStyle(
                  .secondary)
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
      let devices = try await rpc.call("ListPairings").decode([PairedDevice].self)
      guard active, requestID == request, !Task.isCancelled else { return }
      self.devices = devices.filter(\.isPaired).sorted { $0.createdAt > $1.createdAt }
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
      // Like desktop, a new invitation replaces the previous unredeemed one.
      if let previous = invitation, !devices.contains(where: { $0.id == previous.pairID }) {
        _ = try? await rpc.call("RevokePairing", ["pairId": .string(previous.pairID)])
      }
      let result = try await rpc.call(
        "CreatePairing", ["label": .string(PairingInvitation.label())])
      guard active, requestID == request, !Task.isCancelled else { return }
      let invitation = try PairingInvitation(result)
      self.invitation = invitation
      let filter = CIFilter.qrCodeGenerator()
      filter.message = Data(invitation.link.absoluteString.utf8)
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
      _ = try await rpc.call("RevokePairing", ["pairId": .string(device.id)])
      guard active, requestID == request, !Task.isCancelled else { return }
      devices.removeAll { $0.id == device.id }
    } catch is CancellationError {} catch {
      if requestID == request { failure = error.localizedDescription }
    }
  }
}
