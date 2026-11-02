import SwiftUI

#if os(iOS)
  import AVFoundation
  import VisionKit

  struct QRScanner: UIViewControllerRepresentable {
    let scanned: (String) -> Void
    var onError: (String) -> Void = { _ in }
    func makeCoordinator() -> Coordinator { Coordinator(scanned, onError) }
    func makeUIViewController(context: Context) -> DataScannerViewController {
      let controller = DataScannerViewController(
        recognizedDataTypes: [.barcode(symbologies: [.qr])], qualityLevel: .balanced,
        recognizesMultipleItems: false, isHighFrameRateTrackingEnabled: false,
        isPinchToZoomEnabled: true, isGuidanceEnabled: true, isHighlightingEnabled: true)
      controller.delegate = context.coordinator
      do { try controller.startScanning() } catch {
        Task { @MainActor in onError(error.localizedDescription) }
      }
      return controller
    }
    func updateUIViewController(_ controller: DataScannerViewController, context: Context) {}
    static func dismantleUIViewController(
      _ controller: DataScannerViewController, coordinator: Coordinator
    ) { controller.stopScanning() }
    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
      let scanned: (String) -> Void
      let onError: (String) -> Void
      var delivered = false
      init(_ scanned: @escaping (String) -> Void, _ onError: @escaping (String) -> Void) {
        self.scanned = scanned
        self.onError = onError
      }
      func dataScanner(
        _ dataScanner: DataScannerViewController,
        becameUnavailableWithError error: DataScannerViewController.ScanningUnavailable
      ) { onError(error.localizedDescription) }
      func dataScanner(
        _ dataScanner: DataScannerViewController, didAdd addedItems: [RecognizedItem],
        allItems: [RecognizedItem]
      ) {
        guard !delivered else { return }
        for item in addedItems {
          if case .barcode(let barcode) = item, let value = barcode.payloadStringValue {
            delivered = true
            dataScanner.stopScanning()
            scanned(value)
            return
          }
        }
      }
    }
  }

#endif

#if os(iOS)
  import UIKit

  /// Full-screen camera pairing: opens straight to the scanner and pairs on the first code.
  struct PairingScannerView: View {
    let workspace: WorkspaceModel
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    private enum Phase: Equatable {
      case checking, denied, unavailable, scanning, connecting
      case failed(String)
    }
    @State private var phase: Phase = .checking
    @State private var scannerID = UUID()
    @State private var lastValue: String?
    @State private var pairing: Task<Void, Never>?
    @State private var paired = false
    // Browsing asks for local network access while the camera is up, before the pairing request.
    @State private var nearby = NearbyBrowser()

    var body: some View {
      ZStack {
        Color.black.ignoresSafeArea()
        switch phase {
        case .checking:
          ProgressView().tint(.white)
        case .denied:
          permissionPrompt
        case .unavailable:
          message(
            "Camera unavailable",
            detail: "This device can’t scan codes. Paste the pairing link instead.")
        case .scanning, .connecting, .failed:
          QRScanner(
            scanned: submit,
            onError: { phase = .failed($0) }
          ).id(scannerID).ignoresSafeArea()
          RoundedRectangle(cornerRadius: 28, style: .continuous)
            .strokeBorder(AppTheme.accent, lineWidth: 2)
            .aspectRatio(1, contentMode: .fit).padding(.horizontal, 40)
            .allowsHitTesting(false)
        }
      }
      .overlay(alignment: .topLeading) {
        Button {
          dismiss()
        } label: {
          Image(systemName: "xmark").font(.system(size: 17, weight: .semibold))
            .foregroundStyle(.white).frame(width: 44, height: 44)
            .background(.black.opacity(0.55), in: Circle())
        }
        .accessibilityLabel("Close scanner").padding(.leading, 16).padding(.top, 8)
      }
      .overlay(alignment: .bottom) { statusPanel }
      .sensoryFeedback(.success, trigger: paired)
      .task { await checkCamera() }
      .onAppear { nearby.start() }
      .onDisappear {
        nearby.stop()
        pairing?.cancel()
      }
      .preferredColorScheme(.dark)
    }

    @ViewBuilder private var statusPanel: some View {
      VStack(spacing: 12) {
        switch phase {
        case .connecting:
          Text("Connecting to your computer…").font(.subheadline)
          ProgressView().tint(AppTheme.accent)
        case .failed(let error):
          Text("Couldn’t connect").font(.subheadline.weight(.semibold))
          Text(error).font(.footnote).foregroundStyle(.white.opacity(0.75))
            .multilineTextAlignment(.center).textSelection(.enabled)
          HStack(spacing: 10) {
            if lastValue != nil {
              Button("Try Again") { retry() }.buttonStyle(.borderedProminent)
                .tint(AppTheme.accent)
            }
            Button("Scan Again") { rescan() }.buttonStyle(.bordered).tint(.white)
          }.controlSize(.large).buttonBorderShape(.capsule)
        case .scanning, .checking:
          Text("Point your camera at the pairing code on your computer.")
            .font(.subheadline).multilineTextAlignment(.center)
          Text("Zuse desktop → Settings → Devices → Connect another device")
            .font(.caption).foregroundStyle(.white.opacity(0.6)).multilineTextAlignment(.center)
        case .denied, .unavailable:
          EmptyView()
        }
        if phase != .connecting {
          PasteButton(payloadType: String.self) { values in
            if let value = values.first { Task { @MainActor in submit(value) } }
          }
          .buttonBorderShape(.capsule).labelStyle(.titleAndIcon).tint(.white.opacity(0.18))
          .accessibilityLabel("Paste pairing link")
        }
      }
      .foregroundStyle(.white)
      .frame(maxWidth: .infinity).padding(.horizontal, 24).padding(.top, 20).padding(.bottom, 12)
      .background {
        LinearGradient(
          colors: [.black.opacity(0), .black.opacity(0.75)], startPoint: .top, endPoint: .center
        ).ignoresSafeArea()
      }
    }

    private var permissionPrompt: some View {
      VStack(spacing: 14) {
        Image(systemName: "qrcode.viewfinder").font(.system(size: 44)).foregroundStyle(.secondary)
        Text("Camera access needed").font(.title3.weight(.semibold))
        Text("Allow camera access to scan the pairing code shown by Zuse on your computer.")
          .font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
        Button("Open Settings") {
          if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
        }
        .buttonStyle(.borderedProminent).tint(AppTheme.accent).controlSize(.large)
        .buttonBorderShape(.capsule).padding(.top, 6)
      }.padding(.horizontal, 32).foregroundStyle(.white)
    }

    private func message(_ title: String, detail: String) -> some View {
      VStack(spacing: 8) {
        Text(title).font(.title3.weight(.semibold))
        Text(detail).font(.subheadline).foregroundStyle(.secondary)
          .multilineTextAlignment(.center)
      }.padding(.horizontal, 32).foregroundStyle(.white)
    }

    private func checkCamera() async {
      switch AVCaptureDevice.authorizationStatus(for: .video) {
      case .authorized: break
      case .notDetermined:
        guard await AVCaptureDevice.requestAccess(for: .video) else {
          phase = .denied
          return
        }
      default:
        phase = .denied
        return
      }
      phase =
        DataScannerViewController.isSupported && DataScannerViewController.isAvailable
        ? .scanning : .unavailable
    }

    private func submit(_ value: String) {
      guard phase != .connecting else { return }
      lastValue = value
      phase = .connecting
      pairing = Task {
        do {
          try await workspace.pair(value)
          guard !Task.isCancelled else { return }
          paired = true
          dismiss()
        } catch is CancellationError {
        } catch {
          guard !Task.isCancelled else { return }
          phase = .failed(Self.describe(error))
        }
      }
    }

    private func retry() {
      if let lastValue { submit(lastValue) }
    }

    private func rescan() {
      lastValue = nil
      scannerID = UUID()
      phase = .scanning
    }

    private static func describe(_ error: Error) -> String {
      if let error = error as? URLError {
        switch error.code {
        case .timedOut, .cannotConnectToHost, .notConnectedToInternet, .networkConnectionLost,
          .cannotFindHost:
          return
            "Your computer didn’t respond. Make sure this iPhone is on the same Wi-Fi and Local Network access is on in Settings → Zuse Native."
        default: break
        }
      }
      return error.localizedDescription
    }
  }
#endif

extension View {
  /// Presents a connection flow; on iPhone and iPad pairing opens the camera full screen.
  func connectionFlow(_ method: Binding<ConnectionMethod?>, workspace: WorkspaceModel)
    -> some View
  {
    #if os(iOS)
      sheet(
        item: Binding(
          get: { method.wrappedValue == .pairing ? nil : method.wrappedValue },
          set: { method.wrappedValue = $0 })
      ) { ConnectionView(workspace: workspace, method: $0) }
      .fullScreenCover(
        isPresented: Binding(
          get: { method.wrappedValue == .pairing },
          set: { if !$0 { method.wrappedValue = nil } })
      ) { PairingScannerView(workspace: workspace) }
    #else
      sheet(item: method) { ConnectionView(workspace: workspace, method: $0) }
    #endif
  }
}
