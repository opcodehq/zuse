import SwiftUI
import ZuseCore

/// The two views of one pull request, switched from the floating bar.
enum PullRequestTab: String, CaseIterable, Identifiable {
  case conversation, files
  var id: String { rawValue }
  var title: String {
    switch self {
    case .conversation: return "Conversation"
    case .files: return "Files"
    }
  }
  var symbol: String {
    switch self {
    case .conversation: return "text.bubble"
    case .files: return "plus.forwardslash.minus"
    }
  }
}

struct PullRequestDetailView: View {
  let store: PullRequestStore
  let link: PullRequestLink
  /// Shown as a pane beside a chat: the navigation bar belongs to the chat, so the title and
  /// menu stay out of it and checks open in a sheet.
  var embedded = false
  @State private var model: PullRequestDetailModel
  @State private var tab: PullRequestTab = .conversation
  @State private var showsMerge = false
  @State private var showsChecks = false
  @State private var expandedSummary = false
  @State private var draft = ""
  @State private var posting = false
  @State private var commentError: String?
  @State private var notice: String?
  @State private var barHeight: CGFloat = 120
  @FocusState private var composerFocused: Bool

  init(store: PullRequestStore, link: PullRequestLink, embedded: Bool = false) {
    self.store = store
    self.link = link
    self.embedded = embedded
    _model = State(initialValue: PullRequestDetailModel(link: link, store: store))
  }

  var body: some View {
    Group {
      if let detail = model.detail {
        page(detail)
      } else if let error = model.error {
        PullRequestUnavailable(
          title: "Couldn’t open this pull request", message: error,
          symbol: "exclamationmark.triangle"
        ) {
          Task { await model.load(refresh: true) }
        }
      } else {
        PullRequestDetailSkeleton()
      }
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .background(AppTheme.surface)
    .modifier(PullRequestHeader(title: link.shortLabel, enabled: !embedded) { moreMenu })
    .task { await model.load() }
    .task { await model.watch() }
    .task(id: store.previewRevision) {
      if store.preview && store.previewRevision > 0 { await model.load() }
    }
    .onAppear(perform: applyLaunchPreview)
    .sheet(isPresented: $showsMerge) {
      if let detail = model.detail {
        PullRequestMergeSheet(detail: detail) { update, head in
          let refusal = try await model.update(update, head: head)
          if refusal == nil, case .merge = update {
            notice = "Merged into \(detail.baseRefName)."
          }
          return refusal
        }
      }
    }
    .modifier(
      ChecksDestination(presented: $showsChecks, asSheet: embedded) {
        if let detail = model.detail { PullRequestChecksView(detail: detail) }
      })
  }

  // MARK: Page

  private func page(_ detail: PullRequestDetail) -> some View {
    ScrollViewReader { proxy in
      ScrollView {
        VStack(alignment: .leading, spacing: 0) {
          switch tab {
          case .conversation:
            conversation(detail)
          case .files:
            PullRequestFilesView(model: model)
          }
        }
        .frame(maxWidth: tab == .files ? 1200 : 760).frame(maxWidth: .infinity)
        .padding(.bottom, barHeight + 24)
      }
      .scrollDismissesKeyboard(.interactively)
      .refreshable {
        await model.load(refresh: true)
        if tab == .files { await model.loadFiles(force: true) }
      }
      .onAppear {
        #if DEBUG
          if ProcessInfo.processInfo.arguments.contains("--pr-activity-preview") {
            Task {
              try? await Task.sleep(for: .milliseconds(300))
              proxy.scrollTo("activity", anchor: .top)
            }
          }
        #endif
      }
    }
    .overlay(alignment: .bottom) { bottomBar(detail) }
    .onPreferenceChange(PullRequestBarHeight.self) { barHeight = $0 }
    .task(id: "\(tab)|\(model.detail?.headRefOid ?? "")|\(model.detail?.baseRefOid ?? "")") {
      if tab == .files { await model.loadFiles() }
    }
  }

  @ViewBuilder private func conversation(_ detail: PullRequestDetail) -> some View {
    VStack(alignment: .leading, spacing: 14) {
      Text(detail.title.isEmpty ? "Untitled pull request" : detail.title)
        .font(.system(size: 24, weight: .semibold)).tracking(-0.3)
        .fixedSize(horizontal: false, vertical: true)
        .textSelection(.enabled)
        .accessibilityAddTraits(.isHeader)
      HStack(spacing: 6) {
        PullRequestAvatar(actor: detail.author, size: 20)
        Text(detail.author.displayName).font(.system(size: 15, weight: .semibold))
        Text(openedLine(detail)).font(.system(size: 15)).foregroundStyle(.secondary)
      }
      .accessibilityElement(children: .combine)
      facts(detail)
      if let notice { noticeView(notice, tint: PullRequestTint.merged) }
      if let watchError = model.watchError {
        noticeView("Live updates paused: \(watchError)", tint: .secondary)
      }
    }
    .padding(.horizontal, 16).padding(.top, 6)
    VStack(alignment: .leading, spacing: 10) {
      Text("Summary").font(.system(size: 17, weight: .semibold))
      PullRequestSummary(text: detail.body, expanded: $expandedSummary)
    }
    .padding(.horizontal, 16).padding(.top, 26)
    checksRow(detail).padding(.horizontal, 16).padding(.top, 22)
    PullRequestActivityList(detail: detail)
      .id("activity")
      .padding(.horizontal, 16).padding(.top, 12)
  }

  private func openedLine(_ detail: PullRequestDetail) -> String {
    let age = PullRequestAge.ago(store.known[detail.url]?.createdAt)
    switch detail.state {
    case .merged: return "merged into \(detail.baseRefName)"
    case .closed: return "closed this"
    case .open: return age.isEmpty ? "opened this pull request" : "opened \(age)"
    }
  }

  /// State, branches, size and reviewers in one compact card.
  private func facts(_ detail: PullRequestDetail) -> some View {
    PullRequestCard(padding: 12) {
      PullRequestFlow(spacing: 8, lineSpacing: 8) {
        PullRequestChip {
          PullRequestGlyph(state: detail.state, draft: detail.isDraft, size: 12)
          Text(PullRequestStatusLabel.of(detail)).foregroundStyle(.primary)
        }
        PullRequestChip {
          Image(systemName: "arrow.triangle.branch").font(.system(size: 12))
            .foregroundStyle(.secondary)
          Text(detail.headRefName).lineLimit(1).truncationMode(.middle)
          Image(systemName: "arrow.right").font(.system(size: 10, weight: .semibold))
            .foregroundStyle(.tertiary)
          Text(detail.baseRefName).lineLimit(1)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(detail.headRefName) into \(detail.baseRefName)")
        PullRequestChip {
          PullRequestLineCounts(additions: detail.additions, deletions: detail.deletions)
        }
        reviewersChip(detail)
        if let method = detail.autoMergeMethod {
          PullRequestChip {
            Image(systemName: "clock.arrow.circlepath").font(.system(size: 12))
              .foregroundStyle(PullRequestTint.open)
            Text("Auto-merge · \((PullRequestMergeMethod(github: method) ?? .merge).shortName)")
          }
        }
      }
    }
  }

  private func reviewersChip(_ detail: PullRequestDetail) -> some View {
    let reviewers = PullRequestReviewer.from(detail)
    return PullRequestChip {
      if reviewers.isEmpty {
        Image(systemName: "person").font(.system(size: 12)).foregroundStyle(.secondary)
        Text("No reviewers").foregroundStyle(.secondary)
      } else {
        HStack(spacing: -4) {
          ForEach(reviewers.prefix(4)) { reviewer in
            PullRequestAvatar(actor: reviewer.actor, size: 18)
              .overlay(alignment: .bottomTrailing) { verdictDot(reviewer.verdict) }
          }
        }
        Text(reviewerSummary(reviewers)).lineLimit(1)
      }
    }
  }

  @ViewBuilder private func verdictDot(_ verdict: PullRequestReviewer.Verdict) -> some View {
    switch verdict {
    case .approved:
      Image(systemName: "checkmark.circle.fill").font(.system(size: 9, weight: .bold))
        .foregroundStyle(.white, PullRequestTint.open).offset(x: 3, y: 3)
    case .changesRequested:
      Image(systemName: "exclamationmark.circle.fill").font(.system(size: 9, weight: .bold))
        .foregroundStyle(.white, PullRequestTint.deletion).offset(x: 3, y: 3)
    case .commented: EmptyView()
    }
  }

  private func reviewerSummary(_ reviewers: [PullRequestReviewer]) -> String {
    if reviewers.count == 1 { return reviewers[0].actor.displayName }
    let approved = reviewers.filter { $0.verdict == .approved }.count
    return approved > 0
      ? "\(approved) of \(reviewers.count) approved" : "\(reviewers.count) reviewers"
  }

  private func checksRow(_ detail: PullRequestDetail) -> some View {
    let counts = PullRequestChecks(detail.checks)
    let ci = detail.effectiveCI.state
    return Button {
      showsChecks = true
    } label: {
      HStack(spacing: 6) {
        Text("Checks").foregroundStyle(.secondary)
        Image(systemName: "chevron.right").font(.system(size: 11, weight: .semibold))
          .foregroundStyle(.tertiary)
        Spacer()
        PullRequestCheckIcon(
          outcome: counts.failed > 0 || ci == .failed
            ? .failed : counts.pending > 0 || ci == .pending ? .pending : .passed,
          muted: counts.total == 0 && ![.passed, .failed, .pending].contains(ci))
        Text(PullRequestChecks.summary(detail)).foregroundStyle(.secondary).monospacedDigit()
      }
      .font(.system(size: 15))
      .frame(minHeight: 36).contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .disabled(detail.checks.isEmpty)
    .accessibilityLabel("Checks, \(PullRequestChecks.summary(detail))")
  }

  private func noticeView(_ text: String, tint: Color) -> some View {
    Text(text).font(.footnote).foregroundStyle(tint)
      .padding(.horizontal, 12).padding(.vertical, 8)
      .frame(maxWidth: .infinity, alignment: .leading)
      .background(tint.opacity(0.08), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
  }

  // MARK: Floating bar

  private func bottomBar(_ detail: PullRequestDetail) -> some View {
    VStack(spacing: 10) {
      if tab == .conversation { composer(detail) }
      HStack(spacing: 10) {
        tabs
        Spacer(minLength: 8)
        mergeButton(detail)
      }
    }
    .frame(maxWidth: 760).frame(maxWidth: .infinity)
    .padding(.horizontal, 14).padding(.bottom, 8).padding(.top, 8)
    .background {
      GeometryReader { geometry in
        LinearGradient(
          stops: [
            .init(color: AppTheme.surface.opacity(0), location: 0),
            .init(color: AppTheme.surface.opacity(0.8), location: 0.4),
            .init(color: AppTheme.surface.opacity(0.96), location: 1),
          ], startPoint: .top, endPoint: .bottom
        )
        .padding(.top, -28)
        .ignoresSafeArea(edges: .bottom)
        .allowsHitTesting(false)
        .preference(key: PullRequestBarHeight.self, value: geometry.size.height)
      }
    }
  }

  private var tabs: some View {
    HStack(spacing: 2) {
      ForEach(PullRequestTab.allCases) { option in
        Button {
          withAnimation(.snappy(duration: 0.2)) { tab = option }
        } label: {
          Image(systemName: option.symbol).font(.system(size: 16, weight: .medium))
            .frame(width: 50, height: 40)
            .background {
              if tab == option {
                Capsule().fill(Color.primary.opacity(0.09))
              }
            }
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .foregroundStyle(tab == option ? Color.primary : Color.secondary)
        .accessibilityLabel(option.title)
        .accessibilityAddTraits(tab == option ? .isSelected : [])
        .accessibilityIdentifier("pr-tab-\(option.rawValue)")
      }
    }
    .padding(4)
    .floatingGlass(radius: 24)
  }

  @ViewBuilder private func mergeButton(_ detail: PullRequestDetail) -> some View {
    let gate = PullRequestMergeGate.of(detail)
    switch gate {
    case .merged, .closed:
      HStack(spacing: 6) {
        PullRequestGlyph(state: detail.state, size: 14)
        Text(gate == .merged ? "Merged" : "Closed")
      }
      .font(.system(size: 16, weight: .semibold))
      .foregroundStyle(PullRequestTint.state(detail.state, draft: false))
      .padding(.horizontal, 18).frame(height: 48)
      .floatingGlass(radius: 24)
    default:
      Button {
        showsMerge = true
      } label: {
        HStack(spacing: 6) {
          Text(gate == .draft ? "Ready for review" : "Merge")
          Image(systemName: "chevron.down").font(.system(size: 11, weight: .bold))
            .opacity(0.8)
        }
        .font(.system(size: 17, weight: .semibold))
        .lineLimit(1).fixedSize()
        .foregroundStyle(gate.canMerge || gate == .draft ? AppTheme.onAccent : Color.primary)
        .padding(.horizontal, gate == .draft ? 18 : 22).frame(height: 48)
        .background {
          if gate.canMerge || gate == .draft {
            Capsule().fill(AppTheme.accent)
          }
        }
        .modifier(MutedGlass(active: !(gate.canMerge || gate == .draft)))
        .contentShape(Capsule())
      }
      .buttonStyle(.plain)
      .accessibilityIdentifier("pr-merge")
      .accessibilityHint(mergeHint(gate))
    }
  }

  private func mergeHint(_ gate: PullRequestMergeGate) -> String {
    if case .blocked(let blocker) = gate { return "Can’t merge yet: \(blocker.label)" }
    return ""
  }

  private func composer(_ detail: PullRequestDetail) -> some View {
    VStack(alignment: .leading, spacing: 6) {
      if let commentError {
        Text(commentError).font(.caption).foregroundStyle(PullRequestTint.deletion)
          .padding(.horizontal, 6)
      }
      HStack(alignment: .bottom, spacing: 6) {
        TextField("Leave a comment", text: $draft, axis: .vertical)
          .font(.system(size: 16)).lineLimit(1...(composerFocused ? 6 : 1))
          .focused($composerFocused)
          .padding(.leading, 16).padding(.vertical, 12)
          .accessibilityIdentifier("pr-comment")
        if !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || posting {
          Button {
            send()
          } label: {
            ComposerSendGlyph(symbol: "arrow.up", enabled: !posting, busy: posting)
          }
          .buttonStyle(.plain).disabled(posting)
          .padding(.trailing, 4).padding(.bottom, 4)
          .accessibilityLabel("Post comment")
        }
      }
      .frame(minHeight: 46)
      .floatingGlass(radius: 23)
    }
  }

  private func send() {
    let body = draft
    posting = true
    commentError = nil
    Task {
      defer { posting = false }
      do {
        try await model.comment(body)
        // Keep anything typed while it was sending.
        if draft == body { draft = "" }
        composerFocused = false
      } catch {
        commentError = PullRequestErrorText.readable(error)
      }
    }
  }

  private var moreMenu: some View {
    Menu {
      if let url = URL(string: link.url) {
        Link(destination: url) { Label("Open on GitHub", systemImage: "arrow.up.right.square") }
        Button("Copy link", systemImage: "link") { NativePlatform.copy(link.url) }
        ShareLink(item: url)
      }
      Button("Refresh", systemImage: "arrow.clockwise") {
        Task { await model.load(refresh: true) }
      }
    } label: {
      Image(systemName: "ellipsis")
    }
    .tint(.primary)
    .accessibilityLabel("More")
  }

  private func applyLaunchPreview() {
    #if DEBUG
      let arguments = ProcessInfo.processInfo.arguments
      if arguments.contains("--pr-files-preview") { tab = .files }
      if arguments.contains("--pr-expanded-preview") { expandedSummary = true }
      let merge = ["--pr-merge-preview", "--pr-blocked-preview", "--pr-confirm-preview"]
        .contains(where: arguments.contains)
      let checks = arguments.contains("--pr-checks-preview")
      guard merge || checks else { return }
      Task {
        // Open once the pull request has loaded, however long that takes.
        for _ in 0..<240 where model.detail == nil {
          try? await Task.sleep(for: .milliseconds(250))
        }
        try? await Task.sleep(for: .milliseconds(500))
        if merge { showsMerge = true }
        if checks { showsChecks = true }
      }
    #endif
  }
}

/// Glass behind a control that isn't the primary action right now.
private struct MutedGlass: ViewModifier {
  let active: Bool
  func body(content: Content) -> some View {
    if active { content.floatingGlass(radius: 24) } else { content }
  }
}

private struct PullRequestBarHeight: PreferenceKey {
  static let defaultValue: CGFloat = 120
  static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}

extension PullRequestMergeMethod {
  var shortName: String {
    switch self {
    case .squash: return "squash"
    case .merge: return "merge commit"
    case .rebase: return "rebase"
    }
  }
}

/// The description, folded to a few lines until asked for.
struct PullRequestSummary: View {
  let text: String
  @Binding var expanded: Bool
  @State private var fullHeight: CGFloat = 0
  private let foldedHeight: CGFloat = 168
  var body: some View {
    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    if trimmed.isEmpty {
      Text("No description provided.").font(.system(size: 16)).foregroundStyle(.secondary)
    } else {
      VStack(alignment: .leading, spacing: 10) {
        PullRequestMarkdownText(markdown: trimmed)
          .fixedSize(horizontal: false, vertical: true)
          .onGeometryChange(for: CGFloat.self) {
            $0.size.height
          } action: {
            fullHeight = $0
          }
          .frame(maxHeight: folds && !expanded ? foldedHeight : nil, alignment: .top)
          .clipped()
          .mask {
            if folds && !expanded {
              LinearGradient(
                stops: [.init(color: .black, location: 0.6), .init(color: .clear, location: 1)],
                startPoint: .top, endPoint: .bottom)
            } else {
              Rectangle()
            }
          }
        if folds {
          Button {
            withAnimation(.snappy(duration: 0.25)) { expanded.toggle() }
          } label: {
            HStack(spacing: 4) {
              Text(expanded ? "Show less" : "Show more")
              Image(systemName: expanded ? "chevron.up" : "chevron.down")
                .font(.system(size: 11, weight: .semibold))
            }
            .font(.system(size: 15, weight: .medium))
            .foregroundStyle(AppTheme.accent)
          }
          .buttonStyle(.plain)
        }
      }
    }
  }
  private var folds: Bool { fullHeight > foldedHeight + 40 }
}

/// Placeholder bars while the first read is in flight.
struct PullRequestDetailSkeleton: View {
  var body: some View {
    VStack(alignment: .leading, spacing: 14) {
      bar(width: 300, height: 22)
      bar(width: 200, height: 22)
      bar(width: 160, height: 14).padding(.bottom, 8)
      RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Color.primary.opacity(0.05))
        .frame(height: 110)
      ForEach(0..<4, id: \.self) { index in
        bar(width: index == 3 ? 180 : 340, height: 13)
      }
      Spacer()
    }
    .padding(16).frame(maxWidth: 760, alignment: .leading).frame(maxWidth: .infinity)
    .accessibilityLabel("Loading pull request")
  }
  private func bar(width: CGFloat, height: CGFloat) -> some View {
    RoundedRectangle(cornerRadius: height / 2, style: .continuous)
      .fill(Color.primary.opacity(0.07)).frame(maxWidth: width).frame(height: height)
  }
}

/// The pull request's title and menu in the navigation bar, unless it is a pane in a chat.
private struct PullRequestHeader<Menu: View>: ViewModifier {
  let title: String
  let enabled: Bool
  @ViewBuilder let menu: Menu
  func body(content: Content) -> some View {
    if enabled {
      content
        .navigationTitle(title)
        .compactNavigationTitle()
        .toolbar {
          ToolbarItem(placement: .principal) {
            Text(title).font(.system(size: 17, weight: .semibold)).monospacedDigit()
          }
          ToolbarItem(placement: .primaryAction) { menu }
        }
    } else {
      content
    }
  }
}

/// Checks push onto the pull request's stack, or open in a sheet from a chat's pane.
private struct ChecksDestination<Checks: View>: ViewModifier {
  @Binding var presented: Bool
  let asSheet: Bool
  @ViewBuilder let checks: Checks
  func body(content: Content) -> some View {
    if asSheet {
      content.sheet(isPresented: $presented) {
        NavigationStack {
          checks.toolbar {
            ToolbarItem(placement: .confirmationAction) { Button("Done") { presented = false } }
          }
        }
        .nativeSheetSize(width: 620, height: 640)
      }
    } else {
      content.navigationDestination(isPresented: $presented) { checks }
    }
  }
}
