import SwiftUI
import ZuseCore

/// Merge options for one pull request: pick a method, confirm, and see GitHub's answer. When it
/// can't merge yet, says why and offers what can be done instead.
struct PullRequestMergeSheet: View {
  let detail: PullRequestDetail
  /// Sends an update against the confirmed head; returns GitHub's reason when it declined.
  let perform: (PullRequestUpdate, String) async throws -> String?
  @Environment(\.dismiss) private var dismiss
  @State private var confirming: PullRequestUpdate?
  /// The head on screen when the confirmation opened: the only commit it may act on.
  @State private var confirmedHead: String?
  @State private var working = false
  @State private var failure: String?
  @State private var height: CGFloat = 340

  var body: some View {
    VStack(spacing: 0) {
      header
      VStack(alignment: .leading, spacing: 14) {
        if let confirming {
          confirmation(confirming)
        } else {
          options
        }
        if let failure {
          Label(failure, systemImage: "exclamationmark.triangle.fill")
            .font(.system(size: 14)).foregroundStyle(PullRequestTint.deletion)
            .fixedSize(horizontal: false, vertical: true)
            .padding(12).frame(maxWidth: .infinity, alignment: .leading)
            .background(
              PullRequestTint.deletion.opacity(0.08),
              in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        }
      }
      .padding(.horizontal, 18).padding(.bottom, 20)
    }
    .fixedSize(horizontal: false, vertical: true)
    .onGeometryChange(for: CGFloat.self) {
      $0.size.height
    } action: {
      height = $0
    }
    .presentationDetents([.height(max(220, height))])
    .presentationDragIndicator(.hidden)
    .presentationCornerRadius(32)
    .presentationBackground(AppTheme.surface)
    .interactiveDismissDisabled(working)
    .animation(.snappy(duration: 0.22), value: confirming)
    .onChange(of: confirming) { _, value in confirmedHead = value == nil ? nil : detail.headRefOid }
    .onChange(of: detail.headRefOid) {
      // New commits void a confirmation the user hasn't sent yet.
      guard confirming != nil, !working else { return }
      confirming = nil
      failure = "New commits arrived. Review them before trying again."
    }
    .animation(.snappy(duration: 0.22), value: failure)
    .onAppear {
      #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--pr-confirm-preview") {
          confirming = .merge(.squash)
        }
      #endif
    }
  }

  private var header: some View {
    ZStack {
      Text(title).font(.system(size: 17, weight: .semibold))
      HStack {
        Button {
          if confirming != nil && !working {
            confirming = nil
            failure = nil
          } else {
            dismiss()
          }
        } label: {
          Image(systemName: confirming == nil ? "xmark" : "chevron.left")
            .font(.system(size: 15, weight: .semibold)).foregroundStyle(.primary)
            .frame(width: 40, height: 40).floatingGlass(radius: 20)
        }
        .buttonStyle(.plain).disabled(working)
        .accessibilityLabel(confirming == nil ? "Close" : "Back")
        Spacer()
      }
    }
    .padding(.horizontal, 16).padding(.top, 16).padding(.bottom, 14)
  }

  private var title: String {
    switch PullRequestMergeGate.of(detail) {
    case .draft: return "Draft"
    case .ready: return "Merge"
    default: return confirming == nil ? "Can’t merge yet" : "Merge"
    }
  }

  // MARK: Options

  @ViewBuilder private var options: some View {
    let gate = PullRequestMergeGate.of(detail)
    switch gate {
    case .ready:
      if let method = detail.autoMergeMethod.flatMap(PullRequestMergeMethod.init(github:)) {
        autoMergeArmed(method)
      }
      methodList(PullRequestMergeMethod.allCases) { confirming = .merge($0) }
    case .blocked(let blocker):
      reason(blocker.label, blocker.explanation(base: detail.baseRefName), blocker: blocker)
      if let method = detail.autoMergeMethod.flatMap(PullRequestMergeMethod.init(github:)) {
        autoMergeArmed(method)
      } else if PullRequestMergeGate.offersAutoMerge(detail) {
        Text("Merge automatically when ready").font(.system(size: 13, weight: .medium))
          .foregroundStyle(.secondary).padding(.horizontal, 4).padding(.top, 4)
        methodList(PullRequestMergeMethod.allCases) { confirming = .enableAutoMerge($0) }
      }
    case .draft:
      reason(
        "This pull request is a draft",
        "Mark it ready for review to ask for reviews and allow merging.", blocker: nil)
      primaryButton("Ready for review") { confirming = .markReady }
    case .merged, .closed:
      reason(
        gate == .merged ? "Already merged" : "Closed",
        gate == .merged
          ? "This pull request was merged into \(detail.baseRefName)."
          : "Reopen it on GitHub to merge it.", blocker: nil)
    }
  }

  private func methodList(
    _ methods: [PullRequestMergeMethod], choose: @escaping (PullRequestMergeMethod) -> Void
  ) -> some View {
    VStack(spacing: 0) {
      ForEach(Array(methods.enumerated()), id: \.element) { index, method in
        if index > 0 { Divider().padding(.leading, 18) }
        Button {
          failure = nil
          choose(method)
        } label: {
          HStack {
            VStack(alignment: .leading, spacing: 3) {
              Text(method.title).font(.system(size: 17, weight: .medium)).foregroundStyle(.primary)
              Text(method.subtitle).font(.system(size: 14)).foregroundStyle(.secondary)
            }
            Spacer()
            Image(systemName: "chevron.right").font(.system(size: 12, weight: .semibold))
              .foregroundStyle(.tertiary)
          }
          .padding(.horizontal, 18).padding(.vertical, 13).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("merge-method-\(method.rawValue)")
      }
    }
    .background(AppTheme.card, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
  }

  private func reason(_ title: String, _ message: String, blocker: PullRequestBlocker?)
    -> some View
  {
    HStack(alignment: .top, spacing: 12) {
      Image(systemName: symbol(blocker)).font(.system(size: 18, weight: .medium))
        .foregroundStyle(tint(blocker)).frame(width: 26)
      VStack(alignment: .leading, spacing: 3) {
        Text(title).font(.system(size: 16, weight: .semibold))
        Text(message).font(.system(size: 14)).foregroundStyle(.secondary)
          .fixedSize(horizontal: false, vertical: true)
      }
      Spacer(minLength: 0)
    }
    .padding(16)
    .background(AppTheme.card, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
  }

  private func autoMergeArmed(_ method: PullRequestMergeMethod) -> some View {
    VStack(alignment: .leading, spacing: 10) {
      HStack(spacing: 10) {
        Image(systemName: "clock.arrow.circlepath").foregroundStyle(PullRequestTint.open)
        Text("Auto-merge is on (\(method.shortName))").font(.system(size: 15, weight: .medium))
        Spacer()
      }
      Button("Turn off auto-merge", role: .destructive) { confirming = .disableAutoMerge }
        .font(.system(size: 15, weight: .medium))
    }
    .padding(16)
    .background(AppTheme.card, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
  }

  private func symbol(_ blocker: PullRequestBlocker?) -> String {
    switch blocker {
    case .conflicts: return "arrow.triangle.merge"
    case .failingChecks: return "xmark.circle.fill"
    case .changesRequested: return "exclamationmark.bubble.fill"
    case .pendingChecks: return "clock.fill"
    case .unknownStatus: return "questionmark.circle.fill"
    case nil:
      return detail.state == .merged ? "arrow.triangle.merge" : "pencil.circle.fill"
    }
  }
  private func tint(_ blocker: PullRequestBlocker?) -> Color {
    switch blocker {
    case .conflicts, .failingChecks, .changesRequested: return PullRequestTint.deletion
    case .pendingChecks: return PullRequestTint.attention
    case .unknownStatus: return .secondary
    case nil: return detail.state == .merged ? PullRequestTint.merged : .secondary
    }
  }

  // MARK: Confirmation

  private func confirmation(_ update: PullRequestUpdate) -> some View {
    VStack(alignment: .leading, spacing: 14) {
      VStack(alignment: .leading, spacing: 10) {
        Text(confirmTitle(update)).font(.system(size: 17, weight: .semibold))
        HStack(spacing: 6) {
          Image(systemName: "arrow.triangle.branch").font(.system(size: 12))
            .foregroundStyle(.secondary)
          Text(detail.headRefName).lineLimit(1).truncationMode(.middle)
          Image(systemName: "arrow.right").font(.system(size: 10, weight: .semibold))
            .foregroundStyle(.tertiary)
          Text(detail.baseRefName).lineLimit(1)
        }
        .font(.system(size: 14))
        Text(confirmDetail(update)).font(.system(size: 14)).foregroundStyle(.secondary)
          .fixedSize(horizontal: false, vertical: true)
      }
      .padding(16).frame(maxWidth: .infinity, alignment: .leading)
      .background(AppTheme.card, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
      primaryButton(working ? workingLabel(update) : confirmButton(update), busy: working) {
        run(update)
      }
      .accessibilityIdentifier("merge-confirm")
    }
  }

  private func confirmTitle(_ update: PullRequestUpdate) -> String {
    switch update {
    case .merge(let method): return "\(method.title) #\(detail.number)?"
    case .enableAutoMerge(let method): return "Turn on auto-merge (\(method.shortName))?"
    case .disableAutoMerge: return "Turn off auto-merge?"
    case .markReady: return "Mark #\(detail.number) ready for review?"
    }
  }
  private func confirmDetail(_ update: PullRequestUpdate) -> String {
    let commits = detail.commits?.count
    let count = commits.map { $0 == 1 ? "1 commit" : "\($0) commits" } ?? "Its commits"
    switch update {
    case .merge(.squash):
      return "\(count) will be combined into one commit on \(detail.baseRefName)."
    case .merge(.merge):
      return "\(count) will be added to \(detail.baseRefName) with a merge commit."
    case .merge(.rebase):
      return "\(count) will be replayed onto \(detail.baseRefName) one by one."
    case .enableAutoMerge:
      return "GitHub will merge this once its requirements pass."
    case .disableAutoMerge:
      return "This pull request will stay open until someone merges it."
    case .markReady:
      return "Reviewers will be notified and the pull request can be merged."
    }
  }
  private func confirmButton(_ update: PullRequestUpdate) -> String {
    switch update {
    case .merge(let method): return "Confirm \(method.title.lowercased())"
    case .enableAutoMerge: return "Turn on auto-merge"
    case .disableAutoMerge: return "Turn off auto-merge"
    case .markReady: return "Ready for review"
    }
  }
  private func workingLabel(_ update: PullRequestUpdate) -> String {
    switch update {
    case .merge: return "Merging…"
    default: return "Updating…"
    }
  }

  private func primaryButton(_ title: String, busy: Bool = false, action: @escaping () -> Void)
    -> some View
  {
    Button(action: action) {
      HStack(spacing: 8) {
        if busy { ProgressView().tint(.white).controlSize(.small) }
        Text(title)
      }
      .font(.system(size: 17, weight: .semibold)).foregroundStyle(.white)
      .frame(maxWidth: .infinity).frame(height: 52)
      .background(AppTheme.accent, in: Capsule())
      .contentShape(Capsule())
    }
    .buttonStyle(.plain)
    .disabled(working)
  }

  private func run(_ update: PullRequestUpdate) {
    guard let head = confirmedHead else { return }
    working = true
    failure = nil
    Task {
      defer { working = false }
      do {
        if let refusal = try await perform(update, head) {
          failure = refusal
        } else {
          dismiss()
        }
      } catch {
        // A lost reply may still mean GitHub applied it: never invite a blind retry.
        failure =
          ZapperCommands.outcomeUnknown(error)
          ? "Couldn’t confirm the change. It may already have gone through; refresh before trying again."
          : PullRequestErrorText.readable(error)
      }
    }
  }
}
