import SwiftUI
import ZuseCore

/// Pages pushed from the Settings home.
enum SettingsPage: Hashable {
  case account, help, computers, appearance
}

private let supportAddress = URL(string: "mailto:hi@zuse.sh")!

/// Settings home: who is signed in, the computers you use and what this app is.
struct SettingsView: View {
  @Bindable var workspace: WorkspaceModel
  @Environment(\.dismiss) private var dismiss
  @Environment(\.openURL) private var openURL
  @AppStorage("native.appearance") private var appearance = "system"
  @State private var path: [SettingsPage] = []
  @State private var signingIn = false
  private var version: String {
    let info = Bundle.main.infoDictionary
    let short = info?["CFBundleShortVersionString"] as? String ?? "1.0"
    let build = info?["CFBundleVersion"] as? String ?? ""
    return build.isEmpty ? short : "\(short) (\(build))"
  }
  private var computers: [SettingsComputer] {
    workspace.savedConnections.map { connection in
      SettingsComputer(
        reference: .saved(connection.id), name: connection.displayName,
        status: workspace.statusLine(for: connection), state: workspace.state(for: connection),
        tag: connection.ssh == nil ? nil : "SSH", symbol: "laptopcomputer")
    }
      + workspace.environments.map { environment in
        let state = workspace.state(forEnvironment: environment.id)
        return SettingsComputer(
          reference: .account(environment.id), name: environment.label ?? "Computer",
          status: ComputerStatusText.line(state, route: state == .connected ? "Zuse account" : nil),
          state: state, tag: "Account", symbol: "desktopcomputer")
      }
  }
  var body: some View {
    NavigationStack(path: $path) {
      List {
        Section { profileCard }
        computersSection
        Section("About") {
          NavigationLink(value: SettingsPage.appearance) {
            SettingsRow(
              title: "Appearance", icon: "circle.lefthalf.filled", value: appearance.capitalized)
          }
          NavigationLink(value: SettingsPage.help) {
            SettingsRow(
              title: "Help", subtitle: "Connecting computers, Tailscale and SSH",
              icon: "questionmark.circle")
          }
          Button {
            openURL(supportAddress)
          } label: {
            SettingsRow(title: "Contact support", icon: "envelope")
          }
          SettingsRow(title: "Version", icon: "info.circle", value: version)
            .accessibilityElement(children: .combine)
        }
      }
      .settingsList()
      .navigationTitle("Settings").compactNavigationTitle()
      .toolbar {
        ToolbarItem(placement: .sheetClose) { SheetCloseButton { dismiss() } }
      }
      .navigationDestination(for: SettingsPage.self) { page in
        switch page {
        case .account: AccountSettingsView(workspace: workspace)
        case .help: HelpSettingsView { path.append(.computers) }
        case .computers: ComputersView(workspace: workspace)
        case .appearance: appearancePage
        }
      }
      .disabled(workspace.clearingAccount)
    }.nativeSheetSize(width: 440, height: 600).tint(.primary)
  }

  @ViewBuilder private var profileCard: some View {
    let account = workspace.account
    if account.signedIn {
      NavigationLink(value: SettingsPage.account) {
        HStack(spacing: 14) {
          AccountAvatar(account: account, size: 48)
          VStack(alignment: .leading, spacing: 2) {
            Text(account.displayName ?? account.email ?? "Zuse account")
              .font(.headline).lineLimit(1)
            Text(
              account.reauthenticationRequired
                ? "Session expired · sign in again"
                : account.displayName != nil
                  ? account.email ?? "Zuse Cloud and linked computers"
                  : "Zuse Cloud and linked computers"
            ).font(.footnote).foregroundStyle(.secondary).lineLimit(2)
          }
        }.padding(.vertical, 6)
      }
    } else {
      HStack(spacing: 14) {
        AccountAvatar(account: account, size: 48)
        VStack(alignment: .leading, spacing: 2) {
          Text("Not signed in").font(.headline).lineLimit(1)
          Text("Sign in for Zuse Cloud and computers linked to your account")
            .font(.footnote).foregroundStyle(.secondary).lineLimit(2)
        }
        Spacer(minLength: 8)
        Button {
          signIn()
        } label: {
          Text(signingIn ? "Signing In…" : "Sign in").font(.subheadline.weight(.semibold))
        }
        .buttonStyle(.borderedProminent).tint(AppTheme.accent).foregroundStyle(.white)
        .buttonBorderShape(.capsule).disabled(signingIn)
        .accessibilityLabel("Sign in to Zuse")
      }
      .padding(.vertical, 6)
      .contentShape(Rectangle())
      .onTapGesture { path.append(.account) }
      .accessibilityAction(named: "Account") { path.append(.account) }
    }
  }

  private var computersSection: some View {
    let all = computers
    return Section("Computers") {
      ForEach(all.prefix(3)) { computer in
        NavigationLink {
          ComputerDetailView(workspace: workspace, reference: computer.reference)
        } label: {
          ComputerRow(
            name: computer.name, status: computer.status, state: computer.state,
            tag: computer.tag, symbol: computer.symbol)
        }
      }
      NavigationLink(value: SettingsPage.computers) {
        SettingsRow(
          title: all.isEmpty ? "Add computer" : "Manage computers",
          subtitle: all.isEmpty
            ? "Scan a code, paste a link or use SSH"
            : all.count > 3 ? "\(all.count) computers" : nil,
          icon: all.isEmpty ? "plus" : "desktopcomputer")
      }.accessibilityIdentifier("settings-computers")
    }
  }

  private var appearancePage: some View {
    List {
      Picker("Theme", selection: $appearance) {
        Text("System").tag("system")
        Text("Light").tag("light")
        Text("Dark").tag("dark")
      }.pickerStyle(.inline).labelsHidden()
    }.settingsList().navigationTitle("Appearance").compactNavigationTitle()
  }

  private func signIn() {
    guard !signingIn else { return }
    signingIn = true
    Task {
      defer { signingIn = false }
      do { try await workspace.signIn() } catch {
        workspace.error = error.localizedDescription
      }
    }
  }
}

private struct SettingsComputer: Identifiable {
  let reference: ComputerReference
  let name: String
  let status: String
  let state: ComputerState
  let tag: String?
  let symbol: String
  var id: ComputerReference { reference }
}

/// Initials on the brand color when signed in, a person otherwise.
struct AccountAvatar: View {
  let account: Account
  let size: CGFloat
  private var initials: String? {
    let words =
      (account.displayName ?? account.email)?
      .split(whereSeparator: { " .@".contains($0) }) ?? []
    let value = words.prefix(2).compactMap { $0.first.map { String($0).uppercased() } }.joined()
    return value.isEmpty ? nil : value
  }
  var body: some View {
    ZStack {
      if account.signedIn, let initials {
        Circle().fill(AppTheme.accent)
        Text(initials).font(.system(size: size * 0.38, weight: .semibold)).foregroundStyle(.white)
      } else {
        Circle().fill(Color.primary.opacity(0.09))
        Image(systemName: "person.fill").font(.system(size: size * 0.42))
          .foregroundStyle(.secondary)
      }
    }
    .frame(width: size, height: size)
    .accessibilityHidden(true)
  }
}

/// The Zuse account: what signing in gives you, or the cloud settings and sign out.
struct AccountSettingsView: View {
  @Bindable var workspace: WorkspaceModel
  @State private var signingIn = false
  @State private var refreshing = false
  @State private var confirmSignOut = false
  @State private var showCloud = false
  var body: some View {
    let account = workspace.account
    List {
      Section {
        VStack(spacing: 6) {
          AccountAvatar(account: account, size: 72)
          Text(account.signedIn ? account.displayName ?? "Zuse account" : "Sign in to Zuse")
            .font(.title2.weight(.semibold)).padding(.top, 8).multilineTextAlignment(.center)
          Text(
            account.signedIn
              ? account.email ?? "Signed in"
              : "One account for Zuse Cloud and every computer you link."
          ).font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity).padding(.vertical, 8)
        .listRowBackground(Color.clear)
        .accessibilityElement(children: .combine)
      }
      if !account.signedIn {
        Section {
          SettingsRow(
            title: "Cloud chats",
            subtitle: "Start chats on hosted computers, even when yours is off.",
            icon: "cloud", subtitleLines: nil)
          SettingsRow(
            title: "Linked computers",
            subtitle: "Reach computers linked with zuse link from anywhere.",
            icon: "laptopcomputer",
            subtitleLines: nil)
        }
        Section {
          Button {
            signIn()
          } label: {
            Text(signingIn ? "Signing In…" : "Sign in").font(.body.weight(.semibold))
              .frame(maxWidth: .infinity).frame(height: 30)
          }
          .buttonStyle(.borderedProminent).tint(AppTheme.accent).foregroundStyle(.white)
          .controlSize(.large).buttonBorderShape(.capsule).disabled(signingIn)
        }
        .listRowBackground(Color.clear)
        .listRowInsets(EdgeInsets(top: 4, leading: 0, bottom: 4, trailing: 0))
      } else {
        if account.reauthenticationRequired {
          Section {
            Button {
              signIn()
            } label: {
              SettingsRow(
                title: "Sign in again", subtitle: "Your session expired", icon: "arrow.clockwise",
                brand: true)
            }.disabled(signingIn)
          }
        }
        Section {
          Button {
            showCloud = true
          } label: {
            SettingsRow(
              title: "Cloud agents", subtitle: "Claude, Codex and Grok for cloud chats",
              icon: "cloud")
          }
          Button {
            refresh()
          } label: {
            SettingsRow(
              title: refreshing ? "Refreshing…" : "Refresh account",
              subtitle: "Linked computers and cloud chats", icon: "arrow.clockwise")
          }.disabled(refreshing)
        }
        Section {
          Button {
            confirmSignOut = true
          } label: {
            SettingsRow(
              title: "Sign out", icon: "rectangle.portrait.and.arrow.right", destructive: true)
          }
        }
      }
    }
    .settingsList()
    .navigationTitle("Account").compactNavigationTitle()
    .confirmationDialog("Sign out?", isPresented: $confirmSignOut, titleVisibility: .visible) {
      Button("Sign out", role: .destructive) {
        Task { await workspace.signOut() }
      }
    } message: {
      Text(
        "Cloud chats and linked computers disappear from this device until you sign in again. Paired computers stay."
      )
    }
    .sheet(isPresented: $showCloud) { CloudSettingsView(workspace: workspace).nativeSheetSize() }
  }
  private func signIn() {
    guard !signingIn else { return }
    signingIn = true
    Task {
      defer { signingIn = false }
      do { try await workspace.signIn() } catch {
        workspace.error = error.localizedDescription
      }
    }
  }
  private func refresh() {
    guard !refreshing else { return }
    refreshing = true
    Task {
      defer { refreshing = false }
      await workspace.refreshCloud()
    }
  }
}

/// How Zuse reaches your computers, with the commands to set one up.
struct HelpSettingsView: View {
  let addComputer: () -> Void
  @Environment(\.openURL) private var openURL
  var body: some View {
    List {
      Section {
        SettingsRow(
          title: "Your computer",
          subtitle: "Chats run on the computer you connect. Projects are its repositories.",
          icon: "laptopcomputer", subtitleLines: nil)
        SettingsRow(
          title: "At home or away",
          subtitle:
            "Zuse picks the fastest route: your Wi-Fi, Tailscale or Headscale, or SSH. Everything stays end-to-end encrypted.",
          icon: "wifi", subtitleLines: nil)
        SettingsRow(
          title: "Cloud chats",
          subtitle: "A cloud workspace is a hosted computer running a repository and agent.",
          icon: "cloud", subtitleLines: nil)
      }
      Section("On a server or a computer without a screen") {
        HelpCommandRow(command: "zuse serve --lan")
        HelpCommandRow(command: "zuse pair --app")
      }
      Section("Reach it through your account") {
        HelpCommandRow(command: "zuse link")
      }
      Section {
        Button(action: addComputer) {
          SettingsRow(title: "Add a computer", icon: "plus")
        }
        Button {
          openURL(supportAddress)
        } label: {
          SettingsRow(title: "Contact support", icon: "envelope")
        }
      }
    }
    .settingsList()
    .navigationTitle("Help").compactNavigationTitle()
  }
}

/// A shell command with a copy button.
private struct HelpCommandRow: View {
  let command: String
  @State private var copied = false
  var body: some View {
    HStack(spacing: 8) {
      Text(command).font(.system(.callout, design: .monospaced)).textSelection(.enabled)
        .lineLimit(1)
      Spacer(minLength: 8)
      Button {
        NativePlatform.copy(command)
        copied = true
      } label: {
        Image(systemName: copied ? "checkmark" : "doc.on.doc").font(.system(size: 14))
          .frame(width: 32, height: 28).contentShape(Rectangle())
      }
      .buttonStyle(.borderless).foregroundStyle(.secondary)
      .accessibilityLabel(copied ? "Copied" : "Copy \(command)")
    }
    .task(id: copied) {
      guard copied else { return }
      try? await Task.sleep(for: .seconds(1.5))
      copied = false
    }
  }
}
