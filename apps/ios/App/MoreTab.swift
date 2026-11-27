import SwiftUI
import ZuseCore

/// Pages pushed from the More tab.
enum MoreRoute: Hashable {
  case account, issues, computers, appearance, archived, help
}

/// The More tab: your account, everything across Zappers, your computers and the app's settings.
struct MoreView: View {
  @Bindable var workspace: WorkspaceModel
  /// The wide layout's settings list: its pages open beside it, and Issues and Computers
  /// have their own places in the sidebar.
  var selection: Binding<MoreRoute?>?
  @Environment(\.openURL) private var openURL
  @AppStorage("native.appearance") private var appearance = "system"
  private var version: String {
    let info = Bundle.main.infoDictionary
    let short = info?["CFBundleShortVersionString"] as? String ?? "1.0"
    let build = info?["CFBundleVersion"] as? String ?? ""
    return build.isEmpty ? short : "\(short) (\(build))"
  }
  private var needsYou: Int {
    workspace.zappers.zappers.filter { $0.status != .archived }.reduce(0) {
      $0 + $1.issues.filter { $0.details.status == .needsYou }.count
    }
  }
  private var archivedCount: Int {
    workspace.archivedSessions.filter { !workspace.zappers.keepsChat($0.id) }.count
      + workspace.cloudChats.filter(\.isArchived).count
  }

  var body: some View {
    Group {
      if let selection {
        List(selection: selection) { rows }
      } else {
        List { rows }
      }
    }
    .settingsList()
    .navigationTitle(selection == nil ? "More" : "Settings").compactNavigationTitle()
    .disabled(workspace.clearingAccount)
  }

  @ViewBuilder private var rows: some View {
    Section {
      NavigationLink(value: MoreRoute.account) { accountRow }
        .accessibilityIdentifier("more-account")
    }
    if selection == nil {
      Section {
        NavigationLink(value: MoreRoute.issues) {
          MoreRow(title: "Issues", icon: "circle.dashed") {
            if needsYou > 0 { ZapperNeedsYouBadge(count: needsYou) }
          }
        }
        .accessibilityIdentifier("more-issues")
        NavigationLink(value: MoreRoute.computers) {
          MoreRow(title: "Computers", icon: "desktopcomputer") {
            if let state = workspace.computersIndicator {
              HStack(spacing: 6) {
                ComputerStatusDot(state: state, size: 7)
                Text(workspace.connectedComputer ?? state.shortLabel).foregroundStyle(.secondary)
                  .lineLimit(1)
              }
            }
          }
        }
        .accessibilityIdentifier("settings-computers")
      }
    }
    Section("Settings") {
      NavigationLink(value: MoreRoute.appearance) {
        MoreRow(title: "Appearance", icon: "circle.lefthalf.filled") {
          Text(appearance.capitalized).foregroundStyle(.secondary)
        }
      }
      NavigationLink(value: MoreRoute.archived) {
        MoreRow(title: "Archived chats", icon: "archivebox") {
          if archivedCount > 0 {
            Text("\(archivedCount)").monospacedDigit().foregroundStyle(.secondary)
          }
        }
      }
      NavigationLink(value: MoreRoute.help) {
        MoreRow(title: "Help", icon: "questionmark.circle")
      }
      Button {
        openURL(supportAddress)
      } label: {
        MoreRow(title: "Contact support", icon: "envelope")
      }
      .buttonStyle(.plain)
    }
    Section {
      MoreRow(title: "Version", icon: "info.circle") {
        Text(version).foregroundStyle(.secondary).monospacedDigit()
      }
      .accessibilityElement(children: .combine)
    }
  }

  @ViewBuilder private var accountRow: some View {
    let account = workspace.account
    HStack(spacing: 12) {
      AccountAvatar(account: account, size: 34)
      VStack(alignment: .leading, spacing: 1) {
        Text(account.signedIn ? account.displayName ?? account.email ?? "Zuse account" : "Sign in")
          .font(.body.weight(.medium)).lineLimit(1)
        Text(
          account.reauthenticationRequired
            ? "Session expired · sign in again"
            : account.signedIn
              ? (account.displayName != nil ? account.email ?? "Zuse account" : "Zuse account")
              : "Zuse Cloud and linked computers"
        )
        .font(.footnote).foregroundStyle(.secondary).lineLimit(1)
      }
    }
    .padding(.vertical, 2)
    .accessibilityElement(children: .combine)
  }
}

/// A More row: a plain glyph, a title and an optional trailing value.
struct MoreRow<Trailing: View>: View {
  let title: String
  let icon: String
  @ViewBuilder var trailing: Trailing
  var body: some View {
    HStack(spacing: 14) {
      Image(systemName: icon).font(.system(size: 16)).foregroundStyle(.primary.opacity(0.8))
        .frame(width: 22)
        .accessibilityHidden(true)
      Text(title).foregroundStyle(.primary)
      Spacer(minLength: 8)
      trailing
    }
    .contentShape(Rectangle())
  }
}

extension MoreRow where Trailing == EmptyView {
  init(title: String, icon: String) {
    self.init(title: title, icon: icon) { EmptyView() }
  }
}

/// Light, dark or following the system.
struct AppearanceSettingsView: View {
  @AppStorage("native.appearance") private var appearance = "system"
  var body: some View {
    List {
      Picker("Theme", selection: $appearance) {
        Text("System").tag("system")
        Text("Light").tag("light")
        Text("Dark").tag("dark")
      }.pickerStyle(.inline).labelsHidden()
    }
    .settingsList().navigationTitle("Appearance").compactNavigationTitle()
  }
}

/// Every archived chat, on the open computer and in Zuse Cloud, ready to restore.
struct ArchivedChatsView: View {
  @Bindable var workspace: WorkspaceModel
  /// Shows a restored chat in the Chats tab.
  let open: (SessionRow) -> Void
  var body: some View {
    let sessions = workspace.archivedSessions.filter { !workspace.zappers.keepsChat($0.id) }
    let clouds = workspace.cloudChats.filter(\.isArchived)
    let names = Dictionary(
      workspace.projects.map { ($0.id, $0.name) }, uniquingKeysWith: { first, _ in first })
    List {
      ForEach(sessions) { session in
        ArchivedChatRow(
          title: session.title.isEmpty ? "New chat" : session.title, cloud: false,
          project: names[session.projectId], busy: workspace.restoringChats.contains(session.id)
        ) { reopen in
          Task {
            guard await workspace.restoreChat(session), reopen else { return }
            open(workspace.sessions.first { $0.id == session.id } ?? session)
          }
        }
        .chatsRow()
      }
      ForEach(clouds) { chat in
        ArchivedChatRow(
          title: chat.title.isEmpty ? "Untitled chat" : chat.title, cloud: true,
          project: chat.repositoryDisplayName,
          busy: workspace.cloudRequestsBusy.contains(chat.workspaceId)
        ) { reopen in
          Task {
            guard await workspace.controlCloud(chat, action: .unarchive), reopen,
              let fresh = workspace.cloudChats.first(where: { $0.id == chat.id }), !fresh.isArchived
            else { return }
            workspace.openCloud(fresh)
          }
        }
        .chatsRow()
      }
      if sessions.isEmpty && clouds.isEmpty {
        ZappersEmptyState(
          symbol: "archivebox", title: "No archived chats",
          detail: "Swipe a chat to archive it. It waits here, ready to restore."
        )
        .chatsRow(minHeight: 0).padding(.top, 64)
      }
    }
    .listStyle(.plain)
    .scrollContentBackground(.hidden).background(AppTheme.surface)
    .navigationTitle("Archived chats").compactNavigationTitle()
  }
}
