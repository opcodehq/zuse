import Network
import SwiftUI
import ZuseCore

// MARK: - Status

/// Green connected, amber (gently pulsing) connecting, red unreachable, grey idle.
struct ComputerStatusDot: View {
  let state: ComputerState
  var size: CGFloat = 8
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var dimmed = false
  private var color: Color {
    switch state {
    case .connected: .green
    case .connecting: .orange
    case .unreachable: .red
    case .idle: Color.secondary.opacity(0.45)
    }
  }
  private var pulses: Bool { state == .connecting && !reduceMotion }
  var body: some View {
    Circle().fill(color).frame(width: size, height: size)
      .opacity(pulses && dimmed ? 0.35 : 1)
      .animation(
        pulses ? .easeInOut(duration: 0.9).repeatForever(autoreverses: true) : .default,
        value: dimmed
      )
      .onAppear { dimmed = pulses }
      .onChange(of: pulses) { _, value in dimmed = value }
      .accessibilityHidden(true)
  }
}

extension ComputerState {
  var shortLabel: String {
    switch self {
    case .connected: "Connected"
    case .connecting: "Connecting…"
    case .unreachable: "Can’t reach"
    case .idle: "Not connected"
    }
  }
}

/// The header control that opens Computers, with the open connection's dot.
struct ComputersButton: View {
  let workspace: WorkspaceModel
  let action: () -> Void
  var body: some View {
    Button(action: action) {
      Image(systemName: "desktopcomputer")
        .overlay(alignment: .topTrailing) {
          if let state = workspace.computersIndicator {
            ComputerStatusDot(state: state, size: 7)
              .padding(1.5).background(AppTheme.surface, in: Circle())
              .offset(x: 5, y: -4)
          }
        }
    }
    .accessibilityLabel("Computers")
    .accessibilityValue(workspace.computersIndicator?.shortLabel ?? "")
  }
}

// MARK: - Computers

/// Computers in a sheet of its own, for the Home header.
struct ComputersSheet: View {
  @Bindable var workspace: WorkspaceModel
  @Environment(\.dismiss) private var dismiss
  var body: some View {
    NavigationStack {
      ComputersView(workspace: workspace)
        .toolbar {
          ToolbarItem(placement: .sheetClose) { SheetCloseButton { dismiss() } }
        }
    }.nativeSheetSize(width: 460, height: 600).tint(.primary)
  }
}

enum ComputerReference: Hashable {
  case saved(String)
  case account(String)
}

struct ComputersView: View {
  @Bindable var workspace: WorkspaceModel
  @State private var method: ConnectionMethod?
  private var hasComputers: Bool {
    !workspace.savedConnections.isEmpty || !workspace.environments.isEmpty
  }
  var body: some View {
    List {
      if !hasComputers && !workspace.account.signedIn {
        Section {
          VStack(spacing: 14) {
            Image(systemName: "desktopcomputer").font(.system(size: 36, weight: .light))
              .foregroundStyle(.secondary)
            Text("Connect your computer").font(.title3.weight(.semibold))
            Text("Run your agents on your own computer and pick them up from here.")
              .font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
            addButton.padding(.top, 4)
          }
          .frame(maxWidth: .infinity).padding(.vertical, 28)
        }.listRowBackground(Color.clear)
      } else {
        Section { addButton }.listRowBackground(Color.clear)
          .listRowInsets(EdgeInsets(top: 4, leading: 0, bottom: 4, trailing: 0))
        if !workspace.savedConnections.isEmpty {
          Section("Paired") {
            ForEach(workspace.savedConnections) { connection in
              NavigationLink {
                ComputerDetailView(workspace: workspace, reference: .saved(connection.id))
              } label: {
                ComputerRow(
                  name: connection.displayName, status: workspace.statusLine(for: connection),
                  state: workspace.state(for: connection),
                  tag: connection.ssh == nil ? nil : "SSH",
                  symbol: "laptopcomputer")
              }
            }
          }
        }
        if !workspace.environments.isEmpty {
          Section("Zuse account") {
            ForEach(workspace.environments) { environment in
              NavigationLink {
                ComputerDetailView(workspace: workspace, reference: .account(environment.id))
              } label: {
                let state = workspace.state(forEnvironment: environment.id)
                ComputerRow(
                  name: environment.label ?? "Computer",
                  status: ComputerStatusText.line(
                    state, route: state == .connected ? "Zuse account" : nil),
                  state: state, symbol: "desktopcomputer")
              }
            }
          }
        }
        if workspace.account.signedIn {
          Section {
            ComputerRow(
              name: "Zuse Cloud",
              status: workspace.isCloudActive ? workspace.connectionSummary : "Cloud chats",
              state: workspace.isCloudActive
                ? (workspace.connectionState == "Connected" ? .connected : .connecting) : .idle,
              symbol: "cloud")
          }
        }
      }
      Section {
        NavigationLink {
          ServeSettingsView(workspace: workspace)
        } label: {
          #if os(macOS)
            SettingsRow(
              title: "Share this Mac", subtitle: "Reach it from your other devices",
              icon: "square.and.arrow.up.on.square")
          #else
            SettingsRow(
              title: "Set up a computer", subtitle: "Commands for desktops and servers",
              icon: "terminal")
          #endif
        }
      }
    }
    .settingsList()
    .navigationTitle("Computers").compactNavigationTitle()
    .connectionFlow($method, workspace: workspace)
  }
  private var addButton: some View {
    Button {
      method = .add
    } label: {
      Label("Add computer", systemImage: "plus").font(.body.weight(.semibold))
        .frame(maxWidth: .infinity).frame(height: 30)
    }
    .brandProminentButton().controlSize(.large)
    .buttonBorderShape(.capsule)
  }
}

struct ComputerRow: View {
  let name: String
  let status: String
  let state: ComputerState
  var tag: String? = nil
  let symbol: String
  var body: some View {
    HStack(spacing: 12) {
      Image(systemName: symbol).font(.system(size: 17)).foregroundStyle(.secondary)
        .frame(width: 28)
      VStack(alignment: .leading, spacing: 3) {
        HStack(spacing: 6) {
          Text(name).font(.body.weight(.medium)).lineLimit(1)
          if let tag {
            Text(tag).font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
              .padding(.horizontal, 5).padding(.vertical, 1)
              .overlay(Capsule().strokeBorder(Color.secondary.opacity(0.4), lineWidth: 0.5))
          }
        }
        HStack(spacing: 6) {
          ComputerStatusDot(state: state, size: 7)
          Text(status).font(.footnote).foregroundStyle(.secondary).lineLimit(1)
        }
      }
      Spacer(minLength: 0)
    }
    .padding(.vertical, 4).contentShape(Rectangle())
    .accessibilityElement(children: .combine)
  }
}

// MARK: - Detail

struct ComputerDetailView: View {
  @Bindable var workspace: WorkspaceModel
  let reference: ComputerReference
  @Environment(\.dismiss) private var dismiss
  @State private var renaming = false
  @State private var newName = ""
  @State private var forgetting = false

  private var connection: RemoteConnection? {
    guard case .saved(let id) = reference else { return nil }
    return workspace.savedConnections.first { $0.id == id }
  }
  private var environment: RemoteEnvironment? {
    guard case .account(let id) = reference else { return nil }
    return workspace.environments.first { $0.id == id }
  }
  private var name: String {
    connection?.displayName ?? environment?.label ?? "Computer"
  }
  private var state: ComputerState {
    if let connection { return workspace.state(for: connection) }
    if let environment { return workspace.state(forEnvironment: environment.id) }
    return .idle
  }
  private var isActive: Bool {
    if let connection { return workspace.activeConnectionID == connection.id }
    if let environment { return workspace.activeEnvironmentID == environment.id }
    return false
  }
  private var routeText: String {
    if environment != nil { return "Zuse account" }
    guard let connection else { return "—" }
    if state == .connected, let label = workspace.activeRouteLabel {
      let rtt = connection.ssh?.tunnel == true ? nil : workspace.linkRoute?.rttMs
      return rtt.map { "\(label) · \($0) ms" } ?? label
    }
    return connection.ssh?.tunnel == true ? "SSH tunnel" : connection.lastRoute?.label ?? "—"
  }
  private var tip: String? {
    guard state == .unreachable, let connection else { return nil }
    return ComputerStatusText.unreachableTip(
      route: connection.ssh?.tunnel == true ? nil : connection.expectedRoute, name: name,
      usesSSH: connection.ssh?.tunnel == true)
  }

  var body: some View {
    List {
      Section {
        VStack(alignment: .leading, spacing: 10) {
          Text(name).font(.title2.weight(.semibold)).lineLimit(2)
          HStack(spacing: 6) {
            ComputerStatusDot(state: state, size: 7)
            Text(state.shortLabel).font(.footnote.weight(.medium))
          }
          .padding(.horizontal, 10).padding(.vertical, 5)
          .background(Color.primary.opacity(0.06), in: Capsule())
          if !isActive, let saved = connection?.lastConnectedAt {
            Text("Last connected " + ComputerStatusText.ago(saved)).font(.footnote)
              .foregroundStyle(.secondary)
          } else if isActive, state == .unreachable, let problem = workspace.connectionProblem {
            Text(problem).font(.footnote).foregroundStyle(.secondary).textSelection(.enabled)
          }
        }.padding(.vertical, 6)
      }.listRowBackground(Color.clear)
        .listRowInsets(EdgeInsets(top: 0, leading: 4, bottom: 0, trailing: 4))
      Section {
        LabeledContent("Route", value: routeText)
        LabeledContent(
          "Connected through",
          value: environment != nil ? "Zuse account" : connection?.connectedThrough ?? "—")
        if let key = connection?.ssh?.hostKey {
          VStack(alignment: .leading, spacing: 4) {
            Text("SSH host key")
            Text(key).font(.system(.caption, design: .monospaced)).foregroundStyle(.secondary)
              .textSelection(.enabled)
          }.padding(.vertical, 2)
        }
      }
      if let tip {
        Section {
          Label(tip, systemImage: "lightbulb").font(.subheadline).foregroundStyle(.secondary)
        }
      }
      Section {
        Button {
          reconnect()
        } label: {
          Label(isActive ? "Reconnect" : "Connect", systemImage: "arrow.clockwise")
        }
        if let connection {
          Button {
            newName = connection.displayName
            renaming = true
          } label: {
            Label("Rename", systemImage: "pencil")
          }
          NavigationLink {
            ConnectionCheckView(workspace: workspace, connection: connection)
          } label: {
            Label("Check connection", systemImage: "stethoscope")
          }
        }
      }
      if isActive, state == .connected, let rpc = workspace.rpc {
        Section {
          NavigationLink {
            ComputerAccessView(rpc: rpc)
          } label: {
            Label("Device access", systemImage: "ipad.and.iphone")
          }
        }
      }
      if connection != nil || isActive {
        Section {
          if isActive {
            Button {
              workspace.persistDraft(immediate: true)
              workspace.disconnect(clearSaved: true)
            } label: {
              Label("Disconnect", systemImage: "xmark")
            }
          }
          if connection != nil {
            Button(role: .destructive) {
              forgetting = true
            } label: {
              Label("Forget computer", systemImage: "trash")
            }
          }
        }
      }
    }
    .settingsList().tint(.primary)
    .navigationTitle(name).compactNavigationTitle()
    .alert("Rename computer", isPresented: $renaming) {
      TextField("Name", text: $newName)
      Button("Cancel", role: .cancel) {}
      Button("Save") {
        let trimmed = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        if let connection, !trimmed.isEmpty { workspace.renameComputer(connection, label: trimmed) }
      }
    }
    .confirmationDialog("Forget \(name)?", isPresented: $forgetting, titleVisibility: .visible) {
      Button("Forget computer", role: .destructive) {
        if let connection {
          workspace.forgetComputer(connection)
          dismiss()
        }
      }
    } message: {
      Text("You’ll need a new link or SSH sign-in to connect again.")
    }
  }
  private func reconnect() {
    if let connection {
      if workspace.activeConnectionID == connection.id {
        workspace.reconnect()
      } else {
        workspace.openRemote(connection)
      }
    } else if let environment {
      if workspace.activeEnvironmentID == environment.id {
        workspace.reconnect()
      } else {
        workspace.openEnvironment(environment)
      }
    }
  }
}

// MARK: - Checklists

struct CheckStep: Identifiable, Equatable {
  enum State: Equatable { case pending, running, passed, failed, info }
  let id: String
  var title: String
  var state: State = .pending
  var detail: String?
  var fix: String?
}

struct CheckStepRow: View {
  let step: CheckStep
  var body: some View {
    HStack(alignment: .firstTextBaseline, spacing: 10) {
      Group {
        switch step.state {
        case .pending: Image(systemName: "circle").foregroundStyle(.tertiary)
        case .running: ProgressView().controlSize(.small)
        case .passed: Image(systemName: "checkmark").foregroundStyle(.green)
        case .failed: Image(systemName: "xmark").foregroundStyle(.red)
        case .info: Image(systemName: "minus").foregroundStyle(.secondary)
        }
      }
      .font(.system(size: 14, weight: .semibold)).frame(width: 18)
      VStack(alignment: .leading, spacing: 3) {
        Text(step.title).foregroundStyle(step.state == .pending ? .secondary : .primary)
        if let detail = step.detail {
          Text(detail).font(.footnote).foregroundStyle(.secondary).textSelection(.enabled)
        }
        if let fix = step.fix {
          Text((try? AttributedString(markdown: fix)) ?? AttributedString(fix))
            .font(.footnote).foregroundStyle(step.state == .failed ? Color.red : Color.secondary)
        }
      }
    }
    .padding(.vertical, 2)
    .accessibilityElement(children: .combine)
  }
}

/// A one-shot look at this device's network path.
enum NetworkSnapshot {
  struct Value: Sendable {
    let online: Bool
    let medium: String
    let tunnel: Bool
  }
  private final class Once: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false
    func claim() -> Bool {
      lock.withLock {
        if done { return false }
        done = true
        return true
      }
    }
  }
  static func current() async -> Value {
    await withCheckedContinuation { continuation in
      let monitor = NWPathMonitor()
      let once = Once()
      monitor.pathUpdateHandler = { path in
        guard once.claim() else { return }
        let medium =
          path.usesInterfaceType(.wifi)
          ? "Wi‑Fi"
          : path.usesInterfaceType(.cellular)
            ? "Cellular" : path.usesInterfaceType(.wiredEthernet) ? "Ethernet" : "Network"
        // VPN tunnels, Tailscale included, usually appear as "other" interfaces.
        let tunnel = path.availableInterfaces.contains { $0.type == .other }
        monitor.cancel()
        continuation.resume(
          returning: Value(online: path.status == .satisfied, medium: medium, tunnel: tunnel))
      }
      monitor.start(queue: DispatchQueue(label: "sh.zuse.native.network-check"))
    }
  }
}

struct ConnectionCheckView: View {
  let workspace: WorkspaceModel
  let connection: RemoteConnection
  @State private var steps: [CheckStep] = []
  @State private var running = false
  var body: some View {
    List {
      Section {
        ForEach(steps) { CheckStepRow(step: $0) }
      } footer: {
        Text("Checks run from this device to \(connection.displayName).")
      }
    }
    .settingsList()
    .navigationTitle("Check connection").compactNavigationTitle()
    .toolbar {
      ToolbarItem(placement: .primaryAction) {
        Button("Run again") { Task { await run() } }.disabled(running)
      }
    }
    .task { await run() }
  }
  private func update(_ id: String, _ change: (inout CheckStep) -> Void) {
    guard let index = steps.firstIndex(where: { $0.id == id }) else { return }
    change(&steps[index])
  }
  private func run() async {
    guard !running else { return }
    running = true
    defer { running = false }
    let name = connection.displayName
    let ssh = connection.ssh
    steps = [
      CheckStep(id: "network", title: "This device is online"),
      CheckStep(id: "vpn", title: "VPN or Tailscale"),
    ]
    if let ssh { steps.append(CheckStep(id: "ssh", title: "Sign in to \(ssh.target.display)")) }
    steps += [
      CheckStep(id: "reach", title: "Reach \(name)"),
      CheckStep(id: "rtt", title: "Round trip"),
    ]
    update("network") { $0.state = .running }
    let path = await NetworkSnapshot.current()
    update("network") {
      $0.state = path.online ? .passed : .failed
      $0.detail = path.online ? path.medium : nil
      $0.fix = path.online ? nil : "Turn off Airplane Mode or join a Wi‑Fi network."
    }
    let expectsTailnet = ssh?.tunnel != true && connection.expectedRoute == .tailnet
    update("vpn") {
      $0.state = path.tunnel ? .passed : (expectsTailnet ? .failed : .info)
      $0.detail = path.tunnel ? "A VPN is active" : "No VPN detected"
      $0.fix =
        !path.tunnel && expectsTailnet
        ? "Turn on Tailscale on this phone (Headscale works too)." : nil
    }
    guard path.online else {
      skip(["ssh", "reach", "rtt"])
      return
    }
    if let ssh {
      update("ssh") { $0.state = .running }
      do {
        let session = try await SSHSessionPool.shared.acquire(
          ssh.target, expectedHostKey: ssh.hostKey)
        await SSHSessionPool.shared.release(session)
        update("ssh") {
          $0.state = .passed
          $0.detail = session.hostKey
        }
      } catch {
        update("ssh") {
          $0.state = .failed
          $0.fix = error.localizedDescription
        }
        skip(["reach", "rtt"])
        return
      }
    }
    update("reach") { $0.state = .running }
    do {
      let milliseconds = try await workspace.measureRoundTrip(to: connection)
      update("reach") {
        $0.state = .passed
        $0.detail = workspace.activeRouteLabel
      }
      update("rtt") {
        $0.state = .passed
        $0.detail = "\(milliseconds) ms"
      }
    } catch {
      update("reach") {
        $0.state = .failed
        $0.detail = error.localizedDescription
        $0.fix =
          ComputerStatusText.unreachableTip(
            route: ssh?.tunnel == true ? nil : connection.expectedRoute, name: name,
            usesSSH: ssh?.tunnel == true)
      }
      skip(["rtt"])
    }
  }
  private func skip(_ ids: [String]) {
    for id in ids {
      update(id) {
        $0.state = .info
        $0.detail = "Skipped"
      }
    }
  }
}
