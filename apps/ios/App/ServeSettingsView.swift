import SwiftUI
import ZuseCore

struct ServeSettingsView: View {
  let workspace: WorkspaceModel
  @State private var tailscale = false
  @State private var lan = false
  @State private var busy = false
  @State private var status: JSONValue = .null
  @State private var failure: String?
  @State private var output = ""
  @State private var configuration: ServeConfiguration?
  @AppStorage("native.serveExecutable") private var executable = "zuse"
  var body: some View {
    Form {
      Section("Zuse Serve") {
        Text("Keep agents running on your computer and connect from any of your devices.")
          .font(.callout).foregroundStyle(.secondary)
        #if os(macOS)
          TextField("Executable", text: $executable).verbatimInput().frame(height: 28)
          if status != .null {
            LabeledContent("Computer", value: status["computer"].string ?? "Mac")
            LabeledContent("Service", value: status["service"].string ?? "Unknown")
            LabeledContent("Runtime", value: status["runtimeVersion"].string ?? "Unknown")
            LabeledContent("Reachable", value: status["reachable"].bool ? "Yes" : "No")
            LabeledContent("Account tunnel", value: status["tunnel"].string ?? "Unknown")
          }
          Toggle("Share over Tailscale", isOn: $tailscale)
          Toggle("Allow local network connections", isOn: $lan)
          HStack {
            Button("Refresh") {
              Task { await run(["serve", "status", "--json"], readsStatus: true) }
            }
            Spacer()
            Button("Stop") { Task { await run(["serve", "stop"]) } }
            Button("Start") {
              Task {
                await run(
                  configuration?.startArguments(tailscale: tailscale, lan: lan) ?? [
                    "serve", "start",
                  ])
              }
            }.buttonStyle(.borderedProminent).disabled(configuration == nil)
          }.controlSize(.small).frame(height: 28)
          if status["service"].string == "running" {
            Button("Connect this Mac", systemImage: "laptopcomputer") {
              Task { await connectLocal() }
            }.frame(height: 28)
          }
          if busy { ProgressView("Working…") }
          if !output.isEmpty { Text(output).font(.caption).textSelection(.enabled) }
        #else
          Text("Run this on the computer you want to connect:").font(.callout)
          Text("zuse serve start").font(.system(.callout, design: .monospaced)).textSelection(
            .enabled)
          Button("Copy command", systemImage: "doc.on.doc") {
            NativePlatform.copy("zuse serve start")
          }
          Text(
            "Sign in to the same Zuse account, then choose the computer in Settings. You can also pair with its local or Tailscale link."
          )
          .font(.caption).foregroundStyle(.secondary)
        #endif
        if let failure { Text(failure).foregroundStyle(.red) }
      }
      Section("Tailscale & local connections") {
        Text(
          "For Tailscale, start Serve with --tailscale and pair using its HTTPS link. Both devices must be signed in to your tailnet. For local networking, use --lan and pair on the same network."
        )
        .font(.callout).foregroundStyle(.secondary)
        Button("Copy Tailscale command", systemImage: "doc.on.doc") {
          NativePlatform.copy("zuse serve start --tailscale")
        }
        Button("Copy local network command", systemImage: "doc.on.doc") {
          NativePlatform.copy("zuse serve start --lan")
        }
        Link("Connection setup", destination: URL(string: "https://zuse.sh/docs/remote/tailscale")!)
      }
    }
    .settingsList().navigationTitle("Zuse Serve").compactNavigationTitle()
    .disabled(busy)
    #if os(macOS)
      .task { await run(["serve", "status", "--json"], readsStatus: true) }
    #endif
  }
  #if os(macOS)
    private func run(_ args: [String], readsStatus: Bool = false) async {
      guard !busy else { return }
      busy = true
      failure = nil
      defer { busy = false }
      do {
        let directory = status["dataDir"].string
        status = .null
        configuration = nil
        output = ""
        let scoped = args + (directory.map { ["--data-dir", $0] } ?? [])
        let text = try await ServeCommand.run(executable: executable, arguments: scoped)
        if readsStatus {
          status = try JSONDecoder().decode(JSONValue.self, from: Data(text.utf8))
        } else {
          // Lifecycle output can contain one-time pairing codes. Do not retain it in UI or logs.
          output = args.contains("stop") ? "Service stopped." : "Service started."
          let current = try await ServeCommand.run(
            executable: executable,
            arguments: ["serve", "status", "--json"] + (directory.map { ["--data-dir", $0] } ?? []))
          status = try JSONDecoder().decode(JSONValue.self, from: Data(current.utf8))
        }
        guard status["schemaVersion"].int == 1, let directory = status["dataDir"].string,
          !directory.isEmpty
        else {
          throw ClientError("Refresh service status before starting Serve.")
        }
        let saved = try await Task.detached {
          try ServeRuntimeFile.configuration.read(directory: directory)
        }.value
        let configuration = ServeConfiguration(saved ?? .object([:]))
        self.configuration = configuration
        tailscale = configuration.tailscale
        lan = configuration.lan
      } catch { failure = error.localizedDescription }
    }
    private func connectLocal() async {
      guard !busy else { return }
      busy = true
      failure = nil
      defer { busy = false }
      do {
        // Read the directory reported by the running service, never initialize a competing store.
        guard let directory = status["dataDir"].string, status["reachable"].bool else {
          throw ClientError("Refresh the service status and start Serve before connecting.")
        }
        guard
          let value = try await Task.detached(operation: {
            try ServeRuntimeFile.pairing.read(directory: directory)
          }).value
        else { throw ClientError("Open Serve's pairing link and paste it in Connect a computer.") }
        guard let expiration = value["expiresAt"].string,
          let expiresAt = ServerTimestamp.parse(expiration),
          expiresAt > Date(), let link = value["browserUrl"].string
        else {
          throw ClientError("Open Serve's pairing link and paste it in Connect a computer.")
        }
        try await workspace.pair(link)
      } catch { failure = error.localizedDescription }
    }
  #endif
}
