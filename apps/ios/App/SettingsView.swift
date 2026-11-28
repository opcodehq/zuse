import SwiftUI
import ZuseCore

/// Where Contact support writes to.
let supportAddress = URL(string: "mailto:hi@zuse.sh")!

/// The Zuse account: what signing in gives you, or the cloud settings and sign out.
struct AccountSettingsView: View {
  @Bindable var workspace: WorkspaceModel
  @State private var signingIn = false
  @State private var refreshing = false
  @State private var confirmSignOut = false
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
          .brandProminentButton()
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
