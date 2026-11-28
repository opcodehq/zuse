import SwiftUI
import ZuseCore

// MARK: - Marks

/// An issue's status as a small drawn mark, distinct per status like a tracker's.
struct IssueStatusIcon: View {
  let status: ZapperIssueStatus
  var size: CGFloat = 16
  var body: some View {
    let line = max(1.3, size * 0.09)
    ZStack {
      switch status {
      case .backlog:
        Circle().strokeBorder(
          Color.secondary, style: StrokeStyle(lineWidth: line, dash: [size * 0.13, size * 0.12]))
      case .todo:
        Circle().strokeBorder(Color.secondary, lineWidth: line)
      case .inProgress, .inReview:
        let tint: Color = status == .inProgress ? .yellow : .green
        Circle().strokeBorder(tint, lineWidth: line)
        IssuePie(fraction: status == .inProgress ? 0.5 : 0.75).fill(tint)
          .padding(line * 2)
      case .needsYou, .blocked, .done, .canceled:
        Circle().fill(fill)
        Image(systemName: glyph).font(.system(size: size * 0.5, weight: .heavy))
          .foregroundStyle(.white)
      }
    }
    .frame(width: size, height: size)
    .accessibilityElement()
    .accessibilityLabel(status.label)
  }
  private var fill: Color {
    switch status {
    case .needsYou: return .orange
    case .blocked: return .red
    case .done: return .purple
    default: return Color.secondary.opacity(0.7)
    }
  }
  private var glyph: String {
    switch status {
    case .needsYou: return "exclamationmark"
    case .blocked: return "minus"
    case .done: return "checkmark"
    default: return "xmark"
    }
  }
}

private struct IssuePie: Shape {
  let fraction: Double
  func path(in rect: CGRect) -> Path {
    var path = Path()
    let center = CGPoint(x: rect.midX, y: rect.midY)
    path.move(to: center)
    path.addArc(
      center: center, radius: min(rect.width, rect.height) / 2, startAngle: .degrees(-90),
      endAngle: .degrees(-90 + 360 * fraction), clockwise: false)
    path.closeSubpath()
    return path
  }
}

/// Priority as signal bars; urgent is a filled warning tile.
struct IssuePriorityIcon: View {
  let priority: ZapperIssuePriority
  var size: CGFloat = 14
  var body: some View {
    Group {
      if priority == .urgent {
        RoundedRectangle(cornerRadius: size * 0.22, style: .continuous).fill(Color.red)
          .overlay {
            Image(systemName: "exclamationmark").font(.system(size: size * 0.62, weight: .heavy))
              .foregroundStyle(.white)
          }
      } else {
        HStack(alignment: .bottom, spacing: size * 0.12) {
          ForEach(0..<3, id: \.self) { bar in
            RoundedRectangle(cornerRadius: size * 0.08, style: .continuous)
              .fill(Color.primary.opacity(bar < filled ? 0.62 : 0.16))
              .frame(width: size * 0.22, height: size * (0.42 + 0.26 * Double(bar)))
          }
        }
        .frame(width: size, height: size, alignment: .bottom)
      }
    }
    .frame(width: size, height: size)
    .accessibilityElement()
    .accessibilityLabel(priority.label + " priority")
  }
  private var filled: Int {
    switch priority {
    case .urgent, .high: return 3
    case .normal: return 2
    case .low: return 1
    }
  }
}

/// Who holds an issue: you, the Zapper (its mascot) or one of its threads.
struct IssueOwnerMark: View {
  let owner: ZapperIssueOwner?
  let zapper: Zapper?
  let account: Account
  var size: CGFloat = 18
  var body: some View {
    Group {
      switch owner {
      case .user: AccountAvatar(account: account, size: size)
      case .zapper, .thread:
        if let zapper { ZapperAvatarView(zapper: zapper, size: size) } else { unowned }
      case nil: unowned
      }
    }
    .frame(width: size, height: size)
  }
  private var unowned: some View {
    Circle().strokeBorder(
      Color.secondary.opacity(0.6), style: StrokeStyle(lineWidth: 1, dash: [2, 2])
    )
    .overlay {
      Image(systemName: "person.fill").font(.system(size: size * 0.45))
        .foregroundStyle(.secondary)
    }
  }
}

// MARK: - Rows

/// One issue in a list: status, title, then counts, priority, owner and age.
struct IssueListRow: View {
  let issue: ZapperIssue
  let zapper: Zapper?
  let account: Account
  var body: some View {
    let comments = issue.activity.filter { ["comment", "question"].contains($0.kind) }.count
    HStack(spacing: 10) {
      IssueStatusIcon(status: issue.details.status)
      Text(issue.details.title).font(.system(size: PaneMetrics.titleSize))
        .foregroundStyle(.primary.opacity(0.92)).lineLimit(1)
      Spacer(minLength: 6)
      if comments > 0 {
        HStack(spacing: 2) {
          Image(systemName: "bubble.left").font(.system(size: 11, weight: .medium))
          Text("\(comments)").font(.system(size: 12, weight: .medium)).monospacedDigit()
        }
        .foregroundStyle(.secondary)
        .padding(.horizontal, 5).frame(height: 20)
        .overlay(Capsule().strokeBorder(Color.primary.opacity(0.1), lineWidth: 0.5))
        .accessibilityLabel(comments == 1 ? "1 comment" : "\(comments) comments")
      }
      if !issue.pullRequests.isEmpty {
        AppIcon(name: AppIcon.pullRequest, size: 12, weight: .medium)
          .foregroundStyle(.green).accessibilityLabel("Has a pull request")
      }
      IssuePriorityIcon(priority: issue.details.priority, size: 13)
      IssueOwnerMark(owner: issue.details.currentOwner, zapper: zapper, account: account)
      Text(CompactTime.label(milliseconds: issue.updatedAt)).font(.system(size: 13))
        .foregroundStyle(.secondary).monospacedDigit().frame(minWidth: 24, alignment: .trailing)
    }
    .frame(minHeight: PaneMetrics.rowHeight).contentShape(Rectangle())
    .accessibilityElement(children: .combine)
    .accessibilityHint(issue.code)
  }
}

/// A status group's header: label, count and a fold chevron.
struct IssueSectionHeader: View {
  let status: ZapperIssueStatus
  let count: Int
  let collapsed: Bool
  var body: some View {
    HStack(spacing: 6) {
      Text(status.label).font(.system(size: 14, weight: .medium))
        .foregroundStyle(status == .needsYou ? Color.orange : Color.secondary)
      if !collapsed || !status.terminal {
        Text("\(count)").font(.system(size: 14)).monospacedDigit().foregroundStyle(.tertiary)
      }
      Image(systemName: collapsed ? "chevron.right" : "chevron.down")
        .font(.system(size: 10, weight: .bold)).foregroundStyle(.tertiary)
      Spacer(minLength: 0)
    }
    .frame(minHeight: 34).contentShape(Rectangle())
    .accessibilityElement(children: .ignore)
    .accessibilityLabel("\(status.label), \(count)" + (collapsed ? ", collapsed" : ""))
    .accessibilityAddTraits(.isButton)
  }
}

/// Issue groups for a plain list, each folding away under its header.
struct IssueBoardRows: View {
  let workspace: WorkspaceModel
  let sections: [IssueSection]
  @Binding var collapsed: Set<ZapperIssueStatus>
  /// The issue open beside the list, highlighted in it.
  var selected: String?
  let open: (IssueEntry) -> Void
  var body: some View {
    ForEach(sections) { section in
      let folded = collapsed.contains(section.status)
      Button {
        withAnimation(.snappy(duration: 0.2)) {
          if folded { collapsed.remove(section.status) } else { collapsed.insert(section.status) }
        }
      } label: {
        IssueSectionHeader(status: section.status, count: section.entries.count, collapsed: folded)
      }
      .buttonStyle(.plain).issueListRow().padding(.top, 6)
      if !folded {
        ForEach(section.entries) { entry in
          Button {
            open(entry)
          } label: {
            IssueListRow(
              issue: entry.issue, zapper: workspace.zappers.zapper(entry.zapperID),
              account: workspace.account)
          }
          .buttonStyle(.plain).issueListRow(selected: entry.issue.id == selected)
        }
      }
    }
  }
}

extension View {
  /// Row metrics for the issue lists.
  func issueListRow(selected: Bool = false) -> some View {
    listRowInsets(EdgeInsets(top: 0, leading: 16, bottom: 0, trailing: 16))
      .listRowSeparator(.hidden).listRowBackground(SelectedRowBackground(selected: selected))
  }
}

// MARK: - Issues list

/// Every Zapper's issues in one list: what is yours first, grouped by status.
struct IssuesListView: View {
  let workspace: WorkspaceModel
  /// The issue open beside the list, highlighted in it.
  var selectedIssue: String?
  let push: (ZapperRoute) -> Void
  @State private var scope: IssueScope = .forYou
  @State private var filter = IssueFilter()
  @State private var collapsed: Set<ZapperIssueStatus> = [.done, .canceled]
  @State private var showFilter = false
  private var store: ZapperStore { workspace.zappers }

  var body: some View {
    let entries = IssueBoard.entries(store.zappers.filter { $0.status != .archived })
    let sections = IssueBoard.sections(entries, scope: scope, filter: filter)
    List {
      chips(counts: IssueBoard.counts(entries))
        .issueListRow().padding(.vertical, 4)
      if sections.isEmpty {
        empty(hasIssues: !entries.isEmpty).issueListRow().padding(.top, 56)
      } else {
        IssueBoardRows(
          workspace: workspace, sections: sections, collapsed: $collapsed, selected: selectedIssue
        ) { entry in
          push(.issue(zapper: entry.zapperID, issue: entry.issue.id))
        }
      }
    }
    .listStyle(.plain)
    .environment(\.defaultMinListRowHeight, 32)
    .scrollContentBackground(.hidden).background(AppTheme.surface)
    .navigationTitle("Issues").compactNavigationTitle()
    .toolbar {
      ToolbarItem(placement: .primaryAction) {
        Button("Filter", systemImage: "slider.horizontal.3") { showFilter = true }
          .tint(.primary)
      }
    }
    .sheet(isPresented: $showFilter) {
      IssueFilterSheet(
        zappers: store.zappers, counts: IssueBoard.counts(entries), scope: $scope,
        filter: $filter)
    }
    .onAppear(perform: applyLaunchPreview)
  }

  private func chips(counts: [IssueScope: Int]) -> some View {
    HStack(spacing: 8) {
      Menu {
        Picker("View", selection: $scope) {
          ForEach(IssueScope.allCases, id: \.self) { scope in
            Text(scope.label).badge(counts[scope] ?? 0).tag(scope)
          }
        }
      } label: {
        FilterChip(title: scope.label, prominent: true, showsChevron: true)
      }
      .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden)
      .accessibilityIdentifier("issues-scope")
      Button {
        showFilter = true
      } label: {
        FilterChip(
          title: filter.isEmpty ? "Filter" : "Filter · \(filter.activeCount)",
          systemImage: filter.isEmpty ? "plus" : "line.3.horizontal.decrease",
          prominent: false)
      }
      .buttonStyle(.plain)
      Spacer(minLength: 0)
    }
  }

  @ViewBuilder private func empty(hasIssues: Bool) -> some View {
    VStack(spacing: 10) {
      Image(systemName: hasIssues ? "checkmark.circle" : "checklist")
        .font(.system(size: 34, weight: .light)).foregroundStyle(.tertiary)
      Text(hasIssues ? "Nothing here right now" : "No issues yet").font(.headline)
      Text(
        hasIssues
          ? "Nothing matches this view."
          : "Zappers track their work as issues. They show up here as soon as one plans something."
      )
      .font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
      if hasIssues && (scope != .all || !filter.isEmpty) {
        Button("Show all issues") {
          scope = .all
          filter = IssueFilter()
        }
        .font(.subheadline.weight(.semibold)).tint(AppTheme.accent).padding(.top, 4)
      }
    }
    .frame(maxWidth: .infinity).padding(.horizontal, 24)
  }

  private func applyLaunchPreview() {
    #if DEBUG
      if ProcessInfo.processInfo.arguments.contains("--issues-all-preview") { scope = .all }
      if ProcessInfo.processInfo.arguments.contains("--issue-filter-preview") { showFilter = true }
    #endif
  }
}

/// A rounded pill used for list views and filters.
struct FilterChip: View {
  /// A concrete label colour: sidebar glass would make `.primary` vibrant and washed out.
  #if os(macOS)
    static let ink = Color(nsColor: .labelColor)
  #else
    static let ink = Color(uiColor: .label)
  #endif
  let title: String
  var systemImage: String? = nil
  var prominent = false
  var showsChevron = false
  var body: some View {
    HStack(spacing: 5) {
      if let systemImage {
        Image(systemName: systemImage).font(.system(size: 12, weight: .semibold))
      }
      Text(title).font(.system(size: 15, weight: .medium)).lineLimit(1)
      if showsChevron {
        Image(systemName: "chevron.down").font(.system(size: 10, weight: .bold))
          .opacity(0.8)
      }
    }
    .foregroundStyle(prominent ? AppTheme.surface : Color.primary)
    .padding(.horizontal, 13).frame(height: 34)
    .background(
      prominent ? FilterChip.ink : Color.primary.opacity(0.06), in: Capsule()
    )
    .contentShape(Capsule())
  }
}

// MARK: - Choice sheets

/// One option in a choice sheet.
struct ChoiceOption<Value: Hashable>: Identifiable {
  let value: Value
  let title: String
  var subtitle: String? = nil
  var icon: AnyView? = nil
  var id: Value { value }
}

/// A small sheet with one list of options and a checkmark on the current one.
struct ChoiceSheet<Value: Hashable>: View {
  let title: String
  let options: [ChoiceOption<Value>]
  let selected: Value?
  var sectionTitle: String? = nil
  let choose: (Value) -> Void
  @Environment(\.dismiss) private var dismiss
  var body: some View {
    NavigationStack {
      List {
        Section {
          ForEach(options) { option in
            Button {
              choose(option.value)
              dismiss()
            } label: {
              HStack(spacing: 12) {
                if let icon = option.icon { icon.frame(width: 22) }
                VStack(alignment: .leading, spacing: 1) {
                  Text(option.title).foregroundStyle(.primary)
                  if let subtitle = option.subtitle {
                    Text(subtitle).font(.footnote).foregroundStyle(.secondary)
                  }
                }
                Spacer(minLength: 8)
                if option.value == selected {
                  Image(systemName: "checkmark").font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(AppTheme.accent)
                }
              }
              .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityAddTraits(option.value == selected ? .isSelected : [])
          }
        } header: {
          if let sectionTitle { Text(sectionTitle) }
        }
      }
      .settingsList()
      .navigationTitle(title).compactNavigationTitle()
      .toolbar {
        ToolbarItem(placement: .cancellationAction) { SheetCloseButton { dismiss() } }
      }
    }
    .presentationDetents([.height(CGFloat(options.count) * 50 + 150), .large])
    .presentationDragIndicator(.hidden)
    .nativeSheetSize(width: 380, height: CGFloat(options.count) * 50 + 150)
  }
}

/// Views and filters for the Issues list.
struct IssueFilterSheet: View {
  let zappers: [Zapper]
  let counts: [IssueScope: Int]
  @Binding var scope: IssueScope
  @Binding var filter: IssueFilter
  @Environment(\.dismiss) private var dismiss
  var body: some View {
    NavigationStack {
      List {
        Section("Views") {
          ForEach(IssueScope.allCases, id: \.self) { option in
            Button {
              scope = option
            } label: {
              HStack {
                Text(option.label).foregroundStyle(.primary)
                Spacer()
                if option == scope {
                  Image(systemName: "checkmark").fontWeight(.semibold)
                    .foregroundStyle(AppTheme.accent)
                } else if let count = counts[option], count > 0 {
                  Text("\(count)").monospacedDigit().foregroundStyle(.secondary)
                }
              }
              .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
          }
        }
        Section("All filters") {
          NavigationLink {
            MultiChoiceList(
              title: "Status",
              options: ZapperIssueStatus.board.map { status in
                ChoiceOption(
                  value: status, title: status.label,
                  icon: AnyView(IssueStatusIcon(status: status)))
              }, selection: $filter.statuses)
          } label: {
            filterRow("Status", icon: "circle.dashed", values: filter.statuses.map(\.label))
          }
          NavigationLink {
            MultiChoiceList(
              title: "Priority",
              options: ZapperIssuePriority.allCases.map { priority in
                ChoiceOption(
                  value: priority, title: priority.label,
                  icon: AnyView(IssuePriorityIcon(priority: priority)))
              }, selection: $filter.priorities)
          } label: {
            filterRow(
              "Priority", icon: "chart.bar", values: filter.priorities.map(\.label))
          }
          NavigationLink {
            MultiChoiceList(
              title: "Zapper",
              options: zappers.map { zapper in
                ChoiceOption(
                  value: zapper.id, title: zapper.config.name,
                  icon: AnyView(ZapperAvatarView(zapper: zapper, size: 20)))
              }, selection: $filter.zappers)
          } label: {
            filterRow(
              "Zapper", icon: AppIcon.zapper,
              values: zappers.filter { filter.zappers.contains($0.id) }.map(\.config.name))
          }
        }
        if !filter.isEmpty {
          Section {
            Button("Clear filters", role: .destructive) { filter = IssueFilter() }
          }
        }
      }
      .settingsList()
      .navigationTitle("Filter").compactNavigationTitle()
      .toolbar {
        ToolbarItem(placement: .cancellationAction) { SheetCloseButton { dismiss() } }
      }
    }
    .presentationDetents([.large])
    .nativeSheetSize(width: 420, height: 560)
  }
  private func filterRow(_ title: String, icon: String, values: [String]) -> some View {
    HStack(spacing: 12) {
      AppIcon(name: icon, size: 15).foregroundStyle(.secondary).frame(width: 22)
      Text(title)
      Spacer(minLength: 8)
      if !values.isEmpty {
        Text(values.count == 1 ? values[0] : "\(values.count) selected")
          .foregroundStyle(.secondary).lineLimit(1)
      }
    }
  }
}

/// A list of options where any number can be on.
private struct MultiChoiceList<Value: Hashable>: View {
  let title: String
  let options: [ChoiceOption<Value>]
  @Binding var selection: Set<Value>
  var body: some View {
    List {
      ForEach(options) { option in
        Button {
          if selection.contains(option.value) {
            selection.remove(option.value)
          } else {
            selection.insert(option.value)
          }
        } label: {
          HStack(spacing: 12) {
            if let icon = option.icon { icon.frame(width: 22) }
            Text(option.title).foregroundStyle(.primary)
            Spacer()
            if selection.contains(option.value) {
              Image(systemName: "checkmark").fontWeight(.semibold)
                .foregroundStyle(AppTheme.accent)
            }
          }
          .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
      }
    }
    .settingsList()
    .navigationTitle(title).compactNavigationTitle()
  }
}

// MARK: - Issue detail

/// One issue in full: its chips, description, linked threads and pull requests, and activity,
/// with a comment field that hands an issue waiting on you back to the Zapper.
struct ZapperIssueView: View {
  let workspace: WorkspaceModel
  let zapperID: String
  let issueID: String
  let push: (ZapperRoute) -> Void
  @Environment(\.openURL) private var openURL
  @State private var reply = ""
  @State private var sending = false
  @State private var editing: Editor?
  @State private var addingLabel = false
  @State private var newLabel = ""
  @State private var collapsedThreads = false
  @State private var collapsedRelations = false
  @FocusState private var replyFocused: Bool
  private var store: ZapperStore { workspace.zappers }

  enum Editor: String, Identifiable {
    case status, priority, owner
    var id: String { rawValue }
  }

  var body: some View {
    if let zapper = store.zapper(zapperID),
      let issue = zapper.issues.first(where: { $0.id == issueID || $0.code == issueID })
    {
      content(zapper, issue)
    } else {
      ContentUnavailableView(
        "Issue unavailable", systemImage: "checklist",
        description: Text("This issue is no longer on the Zapper."))
    }
  }

  /// The one place issue screens open a pull request. It opens the link for now; a pull
  /// request page can take over here.
  private func openPullRequest(_ url: URL) {
    openURL(url)
  }

  private func content(_ zapper: Zapper, _ issue: ZapperIssue) -> some View {
    ScrollView {
      VStack(alignment: .leading, spacing: 22) {
        header(zapper, issue)
        if issue.details.status == .needsYou, let question = latestQuestion(issue) {
          questionCard(zapper, question)
        }
        issueText(issue)
        threads(zapper, issue)
        relations(zapper, issue)
        pullRequests(issue)
        activity(zapper, issue)
      }
      .padding(.horizontal, 20).padding(.top, 8).padding(.bottom, 24)
      .frame(maxWidth: 720, alignment: .leading).frame(maxWidth: .infinity)
    }
    .scrollDismissesKeyboard(.interactively)
    .hidesTabBarWhenCompact()
    .background(AppTheme.surface)
    .navigationTitle(issue.code)
    .compactNavigationTitle()
    .toolbar {
      ToolbarItem(placement: .primaryAction) {
        Menu {
          Button {
            push(.overview(zapper.id))
          } label: {
            Label("Open \(zapper.config.name)", image: AppIcon.zapper)
          }
          Button("Copy title", systemImage: "doc.on.doc") {
            NativePlatform.copy(issue.code + " " + issue.details.title)
          }
        } label: {
          Label("More", systemImage: "ellipsis")
        }
        .tint(.primary)
      }
    }
    .safeAreaInset(edge: .bottom) { composer(zapper, issue) }
    .sheet(item: $editing) { editor in sheet(editor, zapper: zapper, issue: issue) }
    .alert("Add label", isPresented: $addingLabel) {
      TextField("Label", text: $newLabel).verbatimInput()
      Button("Cancel", role: .cancel) { newLabel = "" }
      Button("Add") {
        let label = newLabel.trimmingCharacters(in: .whitespacesAndNewlines)
        newLabel = ""
        guard !label.isEmpty, !issue.details.labels.contains(label) else { return }
        var details = issue.details
        details.labels.append(label)
        edit(zapper, issue, details, label: "Label")
      }
    }
    .alert(
      "Couldn’t update \(issue.code)", isPresented: failurePresented(zapper.id)
    ) {
      Button("OK") { store.failure.removeValue(forKey: zapper.id) }
    } message: {
      Text(store.failure[zapper.id] ?? "")
    }
    .onAppear { applyLaunchPreview() }
  }

  // MARK: Header

  private func header(_ zapper: Zapper, _ issue: ZapperIssue) -> some View {
    VStack(alignment: .leading, spacing: 14) {
      VStack(alignment: .leading, spacing: 4) {
        Text(issue.details.title).font(.system(size: 22, weight: .semibold))
          .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
        Text(createdLine(issue)).font(.subheadline).foregroundStyle(.secondary)
      }
      IssueChipFlow(spacing: 6) {
        Button {
          editing = .status
        } label: {
          IssueChip {
            IssueStatusIcon(status: issue.details.status, size: 14)
            Text(zapper.issueStatusLabel(issue))
          }
        }
        .accessibilityIdentifier("issue-status-chip")
        Button {
          editing = .priority
        } label: {
          IssueChip {
            IssuePriorityIcon(priority: issue.details.priority, size: 12)
            Text(issue.details.priority.label)
          }
        }
        Button {
          editing = .owner
        } label: {
          IssueChip {
            IssueOwnerMark(
              owner: issue.details.currentOwner, zapper: zapper, account: workspace.account,
              size: 16)
            Text("Owner").foregroundStyle(.secondary)
            Text(ownerLabel(issue.details.currentOwner, zapper: zapper))
          }
        }
        Button {
          push(.overview(zapper.id))
        } label: {
          IssueChip {
            ZapperAvatarView(zapper: zapper, size: 16)
            Text(zapper.config.name)
          }
        }
        if let due = issue.details.dueDate {
          IssueChip {
            Image(systemName: "calendar").font(.system(size: 12)).foregroundStyle(.secondary)
            Text(due)
          }
        }
        ForEach(issue.details.labels, id: \.self) { label in
          Menu {
            Button("Remove label", systemImage: "minus.circle", role: .destructive) {
              var details = issue.details
              details.labels.removeAll { $0 == label }
              edit(zapper, issue, details, label: "Label")
            }
          } label: {
            IssueChip {
              Circle().fill(Color.secondary.opacity(0.6)).frame(width: 7, height: 7)
              Text(label)
            }
          }
          .menuIndicator(.hidden)
        }
        Menu {
          Button("Status", systemImage: "circle.dashed") { editing = .status }
          Button("Priority", systemImage: "chart.bar") { editing = .priority }
          Button("Owner", systemImage: "person") { editing = .owner }
          Button("Label", systemImage: "tag") { addingLabel = true }
        } label: {
          Image(systemName: "plus").font(.system(size: 13, weight: .semibold))
            .foregroundStyle(.secondary).frame(width: 30, height: 30)
            .background(Color.primary.opacity(0.06), in: Circle())
        }
        .menuIndicator(.hidden)
        .accessibilityLabel("Add to issue")
      }
      .buttonStyle(.plain)
      .disabled(store.busy[zapper.id] != nil)
      .padding(8)
      .frame(maxWidth: .infinity, alignment: .leading)
      .background(AppTheme.card, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
      .overlay {
        RoundedRectangle(cornerRadius: 14, style: .continuous)
          .strokeBorder(Color.primary.opacity(0.06), lineWidth: 0.5)
      }
    }
  }

  private func createdLine(_ issue: ZapperIssue) -> String {
    let age = CompactTime.label(milliseconds: issue.createdAt)
    return age == "now" ? "Created just now" : "Created \(age) ago"
  }

  private func questionCard(_ zapper: Zapper, _ question: ZapperIssueActivity) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      HStack(spacing: 6) {
        ZapperAvatarView(zapper: zapper, size: 18)
        Text("\(zapper.config.name) is asking").font(.subheadline.weight(.semibold))
        Text(CompactTime.label(milliseconds: question.createdAt)).font(.footnote)
          .foregroundStyle(.tertiary)
      }
      Text(question.text).font(.subheadline).textSelection(.enabled)
        .fixedSize(horizontal: false, vertical: true)
      Button("Answer") { replyFocused = true }.font(.subheadline.weight(.semibold))
        .buttonStyle(.borderless).tint(AppTheme.accent)
    }
    .padding(14).frame(maxWidth: .infinity, alignment: .leading)
    .background(Color.orange.opacity(0.09), in: RoundedRectangle(cornerRadius: 14))
    .overlay {
      RoundedRectangle(cornerRadius: 14).strokeBorder(Color.orange.opacity(0.25), lineWidth: 0.5)
    }
  }

  @ViewBuilder private func issueText(_ issue: ZapperIssue) -> some View {
    let description = issue.details.description.trimmingCharacters(in: .whitespacesAndNewlines)
    let done = issue.details.doneCriteria.trimmingCharacters(in: .whitespacesAndNewlines)
    let scope = issue.details.outOfScope.trimmingCharacters(in: .whitespacesAndNewlines)
    if !description.isEmpty || !done.isEmpty || !scope.isEmpty {
      VStack(alignment: .leading, spacing: 14) {
        if !description.isEmpty { ChatMarkdown(text: description) }
        if !done.isEmpty { labelled("Done when", done) }
        if !scope.isEmpty { labelled("Out of scope", scope) }
      }
    }
  }

  private func labelled(_ title: String, _ text: String) -> some View {
    VStack(alignment: .leading, spacing: 4) {
      Text(title).font(.footnote.weight(.semibold)).foregroundStyle(.secondary)
      ChatMarkdown(text: text)
    }
  }

  // MARK: Linked work

  private func sectionHeader(_ title: String, count: Int, collapsed: Binding<Bool>?) -> some View {
    Button {
      withAnimation(.snappy(duration: 0.2)) { collapsed?.wrappedValue.toggle() }
    } label: {
      HStack(spacing: 5) {
        Text(title).font(.system(size: 14, weight: .medium)).foregroundStyle(.secondary)
        if let collapsed {
          Image(systemName: collapsed.wrappedValue ? "chevron.right" : "chevron.down")
            .font(.system(size: 9, weight: .bold)).foregroundStyle(.tertiary)
        }
        Spacer()
        Text("\(count)").font(.system(size: 14)).monospacedDigit().foregroundStyle(.tertiary)
      }
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .disabled(collapsed == nil)
  }

  @ViewBuilder private func threads(_ zapper: Zapper, _ issue: ZapperIssue) -> some View {
    if !issue.threads.isEmpty {
      VStack(alignment: .leading, spacing: 4) {
        sectionHeader("Threads", count: issue.threads.count, collapsed: $collapsedThreads)
        if !collapsedThreads {
          ForEach(issue.threads, id: \.self) { chat in
            let worker = zapper.workers.first { $0.chatId == chat }
            let title = store.title(of: chat, fallback: worker?.objective ?? "Thread")
            Button {
              push(
                .chat(
                  store.session(
                    for: chat, fallbackTitle: title,
                    known: workspace.sessions + workspace.archivedSessions)))
            } label: {
              HStack(spacing: 10) {
                ZapperAvatarView(zapper: zapper, size: 18)
                Text(title).font(.system(size: 16)).foregroundStyle(.primary).lineLimit(1)
                liveDot(worker)
                Spacer(minLength: 4)
                Image(systemName: "chevron.right").font(.system(size: 12, weight: .semibold))
                  .foregroundStyle(.tertiary)
              }
              .frame(minHeight: 40).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(worker?.queued == true)
          }
        }
      }
    }
  }

  @ViewBuilder private func liveDot(_ worker: ZapperWorker?) -> some View {
    switch worker.map({ store.rank(of: $0) }) {
    case .working:
      Circle().fill(Color.green).frame(width: 7, height: 7).accessibilityLabel("Working")
    case .needsYou:
      Circle().fill(Color.orange).frame(width: 7, height: 7).accessibilityLabel("Needs you")
    case .queued:
      Text("Queued").font(.caption).foregroundStyle(.secondary)
    case .idle:
      EmptyView()
    case nil:
      Text("Finished").font(.caption).foregroundStyle(.secondary)
    }
  }

  @ViewBuilder private func relations(_ zapper: Zapper, _ issue: ZapperIssue) -> some View {
    let related = (issue.details.parent.map { [$0] } ?? []) + issue.details.relations
    if !related.isEmpty {
      VStack(alignment: .leading, spacing: 4) {
        sectionHeader("Relations", count: related.count, collapsed: $collapsedRelations)
        if !collapsedRelations {
          ForEach(related, id: \.self) { id in
            let other = zapper.issues.first { $0.id == id }
            Button {
              push(.issue(zapper: zapper.id, issue: id))
            } label: {
              HStack(spacing: 8) {
                IssueStatusIcon(status: other?.details.status ?? .backlog, size: 14)
                Text(id == issue.details.parent ? "Parent" : "Related")
                  .font(.system(size: 15)).foregroundStyle(.secondary)
                Text(other?.code ?? id).font(.system(size: 15)).foregroundStyle(.secondary)
                  .monospacedDigit()
                Text(other?.details.title ?? "").font(.system(size: 15))
                  .foregroundStyle(.primary).lineLimit(1)
                Spacer(minLength: 0)
              }
              .frame(minHeight: 36).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(other == nil)
          }
        }
      }
    }
  }

  @ViewBuilder private func pullRequests(_ issue: ZapperIssue) -> some View {
    if !issue.pullRequests.isEmpty {
      VStack(alignment: .leading, spacing: 4) {
        sectionHeader("Pull requests", count: issue.pullRequests.count, collapsed: nil)
        ForEach(issue.pullRequests, id: \.url) { pull in
          if let url = URL(string: pull.url), url.scheme == "https" {
            Button {
              openPullRequest(url)
            } label: {
              HStack(spacing: 10) {
                AppIcon(name: AppIcon.pullRequest, size: 14, weight: .medium)
                  .foregroundStyle(.green).frame(width: 18)
                Text(pull.title.isEmpty ? pullLabel(pull.url) : pull.title)
                  .font(.system(size: 16)).foregroundStyle(.primary).lineLimit(1)
                Text(pullLabel(pull.url)).font(.system(size: 14)).foregroundStyle(.secondary)
                  .lineLimit(1).layoutPriority(-1)
                Spacer(minLength: 4)
                Image(systemName: "chevron.right").font(.system(size: 12, weight: .semibold))
                  .foregroundStyle(.tertiary)
              }
              .frame(minHeight: 40).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
          }
        }
      }
    }
  }

  @ViewBuilder private func activity(_ zapper: Zapper, _ issue: ZapperIssue) -> some View {
    if !issue.activity.isEmpty {
      VStack(alignment: .leading, spacing: 6) {
        sectionHeader("Activity", count: issue.activity.count, collapsed: nil)
        ForEach(issue.activity) { entry in
          ActivityRow(entry: entry, actor: actorName(entry, zapper: zapper), zapper: zapper)
        }
      }
    }
  }

  // MARK: Editing

  @ViewBuilder private func sheet(_ editor: Editor, zapper: Zapper, issue: ZapperIssue)
    -> some View
  {
    switch editor {
    case .status:
      ChoiceSheet(
        title: "Status",
        options: IssueBoard.choosableStatuses(current: issue.details.status).map { status in
          ChoiceOption(
            value: status, title: status.label, icon: AnyView(IssueStatusIcon(status: status)))
        }, selected: issue.details.status
      ) { status in
        edit(
          zapper, issue, IssueBoard.details(issue.details, movingTo: status), label: "Status")
      }
    case .priority:
      ChoiceSheet(
        title: "Priority",
        options: ZapperIssuePriority.allCases.map { priority in
          ChoiceOption(
            value: priority, title: priority.label,
            icon: AnyView(IssuePriorityIcon(priority: priority)))
        }, selected: issue.details.priority
      ) { priority in
        var details = issue.details
        details.priority = priority
        edit(zapper, issue, details, label: "Priority")
      }
    case .owner:
      ChoiceSheet(
        title: "Owner", options: ownerOptions(zapper, issue),
        selected: OwnerChoice(issue.details.currentOwner)
      ) { choice in
        edit(zapper, issue, IssueBoard.details(issue.details, owner: choice.owner), label: "Owner")
      }
    }
  }

  private func ownerOptions(_ zapper: Zapper, _ issue: ZapperIssue) -> [ChoiceOption<OwnerChoice>] {
    var options: [ChoiceOption<OwnerChoice>] = [
      ChoiceOption(
        value: OwnerChoice(nil), title: "No owner",
        icon: AnyView(IssueOwnerMark(owner: nil, zapper: nil, account: workspace.account))),
      ChoiceOption(
        value: OwnerChoice(.user), title: workspace.account.displayName ?? "You",
        subtitle: workspace.account.displayName == nil ? nil : "You",
        icon: AnyView(AccountAvatar(account: workspace.account, size: 20))),
      ChoiceOption(
        value: OwnerChoice(.zapper), title: zapper.config.name, subtitle: "Zapper",
        icon: AnyView(ZapperAvatarView(zapper: zapper, size: 20))),
    ]
    if case .thread(let chat) = issue.details.currentOwner {
      options.append(
        ChoiceOption(
          value: OwnerChoice(.thread(chatId: chat)),
          title: store.title(
            of: chat, fallback: zapper.workers.first { $0.chatId == chat }?.objective ?? "Thread"),
          subtitle: "Thread", icon: AnyView(ZapperAvatarView(zapper: zapper, size: 20))))
    }
    return options
  }

  private func edit(
    _ zapper: Zapper, _ issue: ZapperIssue, _ details: ZapperIssueDetails, label: String
  ) {
    guard details != issue.details else { return }
    Task {
      await store.perform(.editIssue(issueId: issue.id, details: details), on: zapper, label: label)
    }
  }

  private func composer(_ zapper: Zapper, _ issue: ZapperIssue) -> some View {
    let waiting = issue.details.status == .needsYou
    let canSend = !reply.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !sending
    return HStack(alignment: .bottom, spacing: 8) {
      TextField(
        waiting ? "Answer \(zapper.config.name)…" : "Leave a comment", text: $reply,
        axis: .vertical
      )
      .font(.system(size: 17))
      .lineLimit(1...5).focused($replyFocused)
      .padding(.leading, 16).padding(.vertical, 12)
      .accessibilityIdentifier("zapper-issue-comment")
      if canSend || sending {
        Button {
          send(zapper, issue)
        } label: {
          ComposerSendGlyph(symbol: "arrow.up", enabled: canSend, busy: sending)
        }
        .buttonStyle(.plain).disabled(!canSend)
        .accessibilityLabel(waiting ? "Send answer" : "Send comment")
        .padding(.trailing, 2)
      }
    }
    .frame(minHeight: 48)
    .composerGlass(radius: 24)
    .frame(maxWidth: 720).frame(maxWidth: .infinity)
    .padding(.horizontal, 12).padding(.vertical, 6)
  }

  private func send(_ zapper: Zapper, _ issue: ZapperIssue) {
    let text = reply.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !text.isEmpty else { return }
    // Answering the question threads the reply under it and hands the issue back.
    let replyTo = issue.details.status == .needsYou ? latestQuestion(issue)?.id : nil
    sending = true
    Task {
      let sent = await store.perform(
        .commentIssue(issueId: issue.id, text: text, replyTo: replyTo), on: zapper,
        label: replyTo == nil ? "Comment" : "Answer")
      sending = false
      if sent {
        reply = ""
        replyFocused = false
      }
    }
  }

  // MARK: Helpers

  private func latestQuestion(_ issue: ZapperIssue) -> ZapperIssueActivity? {
    issue.activity.last {
      $0.kind == "question" && !$0.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
  }
  private func ownerLabel(_ owner: ZapperIssueOwner?, zapper: Zapper) -> String {
    switch owner {
    case .zapper: return zapper.config.name
    case .user: return workspace.account.displayName ?? "You"
    case .thread(let chat):
      return store.title(
        of: chat, fallback: zapper.workers.first { $0.chatId == chat }?.objective ?? "Thread")
    case nil: return "None"
    }
  }
  private func actorName(_ entry: ZapperIssueActivity, zapper: Zapper) -> String {
    guard let actor = entry.actor else { return "You" }
    if actor == zapper.coordinatorChatId || zapper.previousCoordinatorChats.contains(actor)
      || actor == zapper.id
    {
      return zapper.config.name
    }
    if let worker = zapper.workers.first(where: { $0.chatId == actor }) {
      return store.title(of: actor, fallback: worker.objective)
    }
    return ZapperMembership.isThreadID(actor)
      ? store.title(of: actor, fallback: "Thread") : zapper.config.name
  }
  private func pullLabel(_ url: String) -> String {
    let parts = url.split(separator: "/")
    guard parts.count >= 5 else { return url }
    return "\(parts[parts.count - 3])#\(parts[parts.count - 1])"
  }
  private func failurePresented(_ id: String) -> Binding<Bool> {
    Binding(
      get: { store.failure[id] != nil }, set: { if !$0 { store.failure.removeValue(forKey: id) } })
  }
  private func applyLaunchPreview() {
    #if DEBUG
      let arguments = ProcessInfo.processInfo.arguments
      if let index = arguments.firstIndex(of: "--issue-sheet"), index + 1 < arguments.count,
        let editor = Editor(rawValue: arguments[index + 1])
      {
        editing = editor
      }
    #endif
  }
}

/// An owner choice that can be "nobody".
struct OwnerChoice: Hashable {
  let owner: ZapperIssueOwner?
  init(_ owner: ZapperIssueOwner?) { self.owner = owner }
}

/// A tappable fact about an issue.
private struct IssueChip<Content: View>: View {
  @ViewBuilder let content: Content
  var body: some View {
    HStack(spacing: 5) { content }
      .font(.system(size: 14)).foregroundStyle(.primary).lineLimit(1)
      .padding(.horizontal, 9).frame(height: 30)
      .background(
        Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 8, style: .continuous)
      )
      .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
  }
}

/// Lays chips out in rows, wrapping when a row is full.
private struct IssueChipFlow: Layout {
  var spacing: CGFloat = 6
  func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
    let width = proposal.width ?? .infinity
    var x: CGFloat = 0
    var y: CGFloat = 0
    var line: CGFloat = 0
    var widest: CGFloat = 0
    for subview in subviews {
      let size = subview.sizeThatFits(.unspecified)
      if x > 0 && x + size.width > width {
        y += line + spacing
        x = 0
        line = 0
      }
      x += size.width + spacing
      line = max(line, size.height)
      widest = max(widest, x - spacing)
    }
    return CGSize(width: min(widest, width), height: y + line)
  }
  func placeSubviews(
    in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()
  ) {
    var x = bounds.minX
    var y = bounds.minY
    var line: CGFloat = 0
    for subview in subviews {
      let size = subview.sizeThatFits(.unspecified)
      if x > bounds.minX && x + size.width > bounds.maxX {
        y += line + spacing
        x = bounds.minX
        line = 0
      }
      subview.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
      x += size.width + spacing
      line = max(line, size.height)
    }
  }
}

private struct ActivityRow: View {
  let entry: ZapperIssueActivity
  let actor: String
  let zapper: Zapper
  var body: some View {
    if isConversation {
      VStack(alignment: .leading, spacing: 6) {
        HStack(spacing: 6) {
          if actor == "You" {
            Image(systemName: "person.crop.circle.fill").font(.system(size: 16))
              .foregroundStyle(.secondary)
          } else {
            ZapperAvatarView(zapper: zapper, size: 16)
          }
          Text(actor).font(.subheadline.weight(.semibold)).lineLimit(1)
          Text(CompactTime.label(milliseconds: entry.createdAt)).font(.footnote)
            .foregroundStyle(.tertiary)
          if entry.kind == "question" {
            Text("Question").font(.caption.weight(.medium)).foregroundStyle(.orange)
          }
        }
        Text(entry.text).font(.subheadline).textSelection(.enabled)
          .fixedSize(horizontal: false, vertical: true)
      }
      .padding(12).frame(maxWidth: .infinity, alignment: .leading)
      .background(AppTheme.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
      .overlay {
        RoundedRectangle(cornerRadius: 12, style: .continuous)
          .strokeBorder(Color.primary.opacity(0.06), lineWidth: 0.5)
      }
      .padding(.leading, entry.replyTo == nil ? 0 : 18)
      .padding(.vertical, 2)
    } else {
      HStack(alignment: .firstTextBaseline, spacing: 10) {
        AppIcon(name: symbol, size: 12, weight: .medium)
          .foregroundStyle(.secondary).frame(width: 18)
        (Text(actor).foregroundStyle(.primary) + Text(" " + summary).foregroundStyle(.secondary))
          .font(.system(size: 14)).lineLimit(2)
        Spacer(minLength: 6)
        Text(CompactTime.label(milliseconds: entry.createdAt)).font(.system(size: 13))
          .foregroundStyle(.tertiary).monospacedDigit()
      }
      .padding(.vertical, 5)
      .accessibilityElement(children: .combine)
    }
  }
  private var isConversation: Bool { ["comment", "question"].contains(entry.kind) }
  /// Notes read as a sentence after the actor; the first line is enough.
  private var summary: String {
    let first =
      entry.text.split(whereSeparator: \.isNewline).first.map(String.init) ?? entry.text
    guard let initial = first.first else { return "" }
    return initial.lowercased() + first.dropFirst()
  }
  private var symbol: String {
    switch entry.kind {
    case "workerReport": return "bubble.left.and.text.bubble.right"
    case "thread": return "bubble.left.and.bubble.right"
    case "pullRequest": return AppIcon.pullRequest
    case "closed": return "checkmark.seal"
    case "created": return "plus"
    default: return "pencil"
    }
  }
}
