import SwiftUI
import ZuseCore

/// Comments, reviews, inline threads and commits, oldest first.
struct PullRequestActivityList: View {
  let detail: PullRequestDetail
  @AppStorage("native.pullRequests.showsCommits") private var showsCommits = true
  var body: some View {
    let items = PullRequestActivity.timeline(detail, commits: showsCommits)
    VStack(alignment: .leading, spacing: 12) {
      HStack {
        Text("Activity").foregroundStyle(.secondary)
        Spacer()
        Menu {
          Toggle("Show commits", systemImage: "circle.and.line.horizontal", isOn: $showsCommits)
        } label: {
          Image(systemName: "line.3.horizontal.decrease").font(.system(size: 15, weight: .medium))
            .foregroundStyle(.secondary).frame(width: 36, height: 30).contentShape(Rectangle())
        }
        .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden)
        .accessibilityLabel("Activity filter")
      }
      .font(.system(size: 15))
      if detail.reviewThreads == nil && !(detail.reviews.isEmpty) {
        Text("Inline review comments are loading…").font(.footnote).foregroundStyle(.tertiary)
      }
      if items.isEmpty {
        Text("No activity yet.").font(.system(size: 15)).foregroundStyle(.tertiary)
          .padding(.vertical, 6)
      }
      ForEach(items) { item in
        switch item {
        case .comment(let comment):
          PullRequestCommentCard(comment: comment)
        case .review(let review):
          PullRequestReviewEvent(review: review)
        case .thread(let thread):
          PullRequestThreadCard(thread: thread)
        case .commit(let commit):
          PullRequestCommitEvent(commit: commit)
        }
      }
    }
  }
}

/// Author, bot tag and age above a body.
struct PullRequestCommentHeader: View {
  let actor: PullRequestActor
  let date: Date?
  var detail: String?
  var body: some View {
    HStack(spacing: 7) {
      PullRequestAvatar(actor: actor, size: 20)
      Text(actor.displayName).font(.system(size: 15, weight: .semibold)).lineLimit(1)
      if actor.isBot {
        Text("bot").font(.system(size: 11, weight: .medium)).foregroundStyle(.secondary)
          .padding(.horizontal, 5).padding(.vertical, 1)
          .background(Color.primary.opacity(0.07), in: Capsule())
      }
      if let detail { Text(detail).font(.system(size: 14)).foregroundStyle(.secondary) }
      Spacer(minLength: 4)
      Text(PullRequestAge.short(date)).font(.system(size: 13)).foregroundStyle(.tertiary)
        .monospacedDigit()
    }
    .accessibilityElement(children: .combine)
  }
}

struct PullRequestCommentCard: View {
  let comment: PullRequestComment
  var body: some View {
    PullRequestCard {
      PullRequestCommentHeader(actor: comment.author, date: comment.date)
      if comment.body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
        Text("No description").font(.system(size: 15)).foregroundStyle(.tertiary)
      } else {
        PullRequestMarkdownText(markdown: comment.body, size: 15.5)
      }
    }
    .contextMenu { PullRequestCommentMenu(comment: comment) }
  }
}

struct PullRequestCommentMenu: View {
  let comment: PullRequestComment
  var body: some View {
    Button("Copy text", systemImage: "doc.on.doc") { NativePlatform.copy(comment.body) }
    if let url = URL(string: comment.url), url.scheme == "https" {
      Link(destination: url) { Label("Open on GitHub", systemImage: "arrow.up.right.square") }
    }
  }
}

/// "sam approved", with the review's summary below when it wrote one.
struct PullRequestReviewEvent: View {
  let review: PullRequestComment
  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      HStack(spacing: 8) {
        icon.font(.system(size: 14, weight: .medium)).frame(width: 20)
        (Text(review.author.displayName).fontWeight(.semibold).foregroundStyle(.primary)
          + Text(" " + PullRequestActivity.reviewVerb(review.state)))
          .font(.system(size: 15)).foregroundStyle(.secondary).lineLimit(1)
        Spacer(minLength: 4)
        Text(PullRequestAge.short(review.date)).font(.system(size: 13))
          .foregroundStyle(.tertiary).monospacedDigit()
      }
      .padding(.horizontal, 4)
      .accessibilityElement(children: .combine)
      if !review.body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
        PullRequestCard {
          PullRequestMarkdownText(markdown: review.body, size: 15.5)
        }
        .contextMenu { PullRequestCommentMenu(comment: review) }
      }
    }
  }
  @ViewBuilder private var icon: some View {
    switch review.state.uppercased() {
    case "APPROVED":
      Image(systemName: "checkmark.circle.fill").foregroundStyle(PullRequestTint.open)
    case "CHANGES_REQUESTED":
      Image(systemName: "exclamationmark.bubble.fill").foregroundStyle(PullRequestTint.deletion)
    default:
      Image(systemName: "eye").foregroundStyle(.secondary)
    }
  }
}

struct PullRequestCommitEvent: View {
  let commit: PullRequestCommit
  var body: some View {
    HStack(spacing: 8) {
      Image(systemName: "circle.and.line.horizontal").font(.system(size: 13))
        .foregroundStyle(.tertiary).frame(width: 20)
      Text(commit.authorName).font(.system(size: 14, weight: .medium)).foregroundStyle(.secondary)
        .lineLimit(1).layoutPriority(1)
      Text(commit.shortOid).font(.system(size: 13, design: .monospaced)).foregroundStyle(.tertiary)
      Text(commit.messageHeadline).font(.system(size: 14)).foregroundStyle(.primary.opacity(0.8))
        .lineLimit(1)
      Spacer(minLength: 4)
      Text(PullRequestAge.short(commit.date)).font(.system(size: 13)).foregroundStyle(.tertiary)
        .monospacedDigit()
    }
    .padding(.horizontal, 4)
    .accessibilityElement(children: .combine)
  }
}

/// An inline conversation: where it points, the code, then the comments in order.
struct PullRequestThreadCard: View {
  let thread: PullRequestReviewThread
  @State private var expanded: Bool?
  var body: some View {
    let open = expanded ?? !thread.isResolved
    PullRequestCard {
      Button {
        withAnimation(.snappy(duration: 0.2)) { expanded = !open }
      } label: {
        HStack(spacing: 6) {
          Text(location).font(.system(size: 13)).foregroundStyle(.secondary).lineLimit(1)
            .truncationMode(.head)
          Spacer(minLength: 4)
          if thread.isOutdated { tag("Outdated", tint: .secondary) }
          if thread.isResolved { tag("Resolved", tint: PullRequestTint.open) }
          Image(systemName: open ? "chevron.up" : "chevron.down")
            .font(.system(size: 11, weight: .semibold)).foregroundStyle(.tertiary)
        }
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      if let root = thread.comments.first {
        let snippet = PullRequestHunkSnippet.lines(root.diffHunk)
        if !snippet.isEmpty && open { PullRequestSnippet(lines: snippet) }
        if open {
          VStack(alignment: .leading, spacing: 14) {
            ForEach(Array(thread.comments.enumerated()), id: \.element.id) { index, comment in
              VStack(alignment: .leading, spacing: 8) {
                PullRequestCommentHeader(actor: comment.author, date: comment.date)
                PullRequestMarkdownText(markdown: comment.body, size: 15)
              }
              .padding(.leading, index == 0 ? 0 : 12)
              .overlay(alignment: .leading) {
                if index > 0 {
                  Capsule().fill(Color.primary.opacity(0.1)).frame(width: 2)
                }
              }
              .contextMenu { PullRequestCommentMenu(comment: comment) }
            }
          }
          .padding(.top, 2)
        } else {
          HStack(spacing: 6) {
            PullRequestAvatar(actor: root.author, size: 16)
            Text(PullRequestMarkdown.firstLine(root.body)).font(.system(size: 14))
              .foregroundStyle(.secondary).lineLimit(1)
            if thread.comments.count > 1 {
              Spacer(minLength: 4)
              Text("\(thread.comments.count - 1) more").font(.system(size: 13))
                .foregroundStyle(.tertiary)
            }
          }
        }
      }
    }
  }
  private var location: String {
    var parts = [thread.path]
    if let line = thread.displayLine {
      if let start = thread.displayStartLine, start != line {
        parts.append("Lines \(start)–\(line)")
      } else {
        parts.append("Line \(line)")
      }
    }
    return parts.joined(separator: " · ")
  }
  private func tag(_ text: String, tint: Color) -> some View {
    Text(text).font(.system(size: 11, weight: .medium)).foregroundStyle(tint)
      .padding(.horizontal, 6).padding(.vertical, 2)
      .background(tint.opacity(0.1), in: Capsule())
  }
}

/// A few numbered diff lines, tinted, scrolling sideways for long lines.
struct PullRequestSnippet: View {
  let lines: [DiffLine]
  var body: some View {
    let width = CGFloat(String(lines.compactMap { $0.newLine ?? $0.oldLine }.max() ?? 0).count) * 8
    HStack(alignment: .top, spacing: 0) {
      VStack(alignment: .trailing, spacing: 0) {
        ForEach(lines) { line in
          Text(number(line)).frame(minWidth: max(24, width), alignment: .trailing)
            .padding(.horizontal, 6).frame(height: 19)
            .background(tint(line).opacity(0.12))
        }
      }
      .foregroundStyle(.secondary)
      PullRequestCodeScroller {
        ForEach(lines) { line in
          Text(marker(line) + " " + line.text).fixedSize()
            .padding(.trailing, 10).frame(height: 19)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(tint(line).opacity(0.12))
        }
      }
    }
    .font(.system(size: 12, design: .monospaced))
    .clipShape(RoundedRectangle(cornerRadius: 9, style: .continuous))
    .background(
      Color.primary.opacity(0.03), in: RoundedRectangle(cornerRadius: 9, style: .continuous)
    )
    .accessibilityElement(children: .ignore)
    .accessibilityLabel("Code: " + lines.map(\.text).joined(separator: "\n"))
  }
  private func number(_ line: DiffLine) -> String {
    (line.newLine ?? line.oldLine).map(String.init) ?? ""
  }
  private func marker(_ line: DiffLine) -> String {
    switch line.kind {
    case .addition: return "+"
    case .deletion: return "−"
    default: return " "
    }
  }
  private func tint(_ line: DiffLine) -> Color {
    switch line.kind {
    case .addition: return PullRequestTint.addition
    case .deletion: return PullRequestTint.deletion
    default: return .clear
    }
  }
}

/// The Commits tab: every commit, newest day first.
struct PullRequestCommitsList: View {
  let detail: PullRequestDetail
  var body: some View {
    let commits = (detail.commits ?? []).sorted {
      ($0.date ?? .distantPast) > ($1.date ?? .distantPast)
    }
    VStack(alignment: .leading, spacing: 12) {
      if detail.commits == nil {
        Text("Commit history is unavailable. Pull to refresh.").font(.system(size: 15))
          .foregroundStyle(.secondary).padding(.top, 20)
      } else if commits.isEmpty {
        Text("No commits to show.").font(.system(size: 15)).foregroundStyle(.secondary)
          .padding(.top, 20)
      }
      let days = Dictionary(grouping: commits) { commit in
        Calendar.current.startOfDay(for: commit.date ?? .distantPast)
      }
      ForEach(days.keys.sorted(by: >), id: \.self) { day in
        VStack(alignment: .leading, spacing: 8) {
          Text(day == Calendar.current.startOfDay(for: .distantPast) ? "Undated" : dayLabel(day))
            .font(.system(size: 13, weight: .medium)).foregroundStyle(.secondary)
            .padding(.horizontal, 4)
          PullRequestCard(padding: 0) {
            VStack(spacing: 0) {
              ForEach(Array((days[day] ?? []).enumerated()), id: \.element.id) { index, commit in
                if index > 0 { Divider().padding(.leading, 14) }
                commitRow(commit)
              }
            }
          }
        }
      }
    }
  }
  private func commitRow(_ commit: PullRequestCommit) -> some View {
    VStack(alignment: .leading, spacing: 4) {
      Text(commit.messageHeadline.isEmpty ? "No message" : commit.messageHeadline)
        .font(.system(size: 15, weight: .medium)).lineLimit(2)
      HStack(spacing: 6) {
        Text(commit.shortOid).font(.system(size: 12, design: .monospaced))
          .padding(.horizontal, 5).padding(.vertical, 1)
          .background(Color.primary.opacity(0.06), in: RoundedRectangle(cornerRadius: 5))
        Text(commit.authorName + " · " + PullRequestAge.short(commit.date))
          .font(.system(size: 13)).foregroundStyle(.secondary).lineLimit(1)
        Spacer(minLength: 4)
        if let additions = commit.additions, let deletions = commit.deletions {
          PullRequestLineCounts(additions: additions, deletions: deletions, size: 12)
        }
      }
    }
    .padding(.horizontal, 14).padding(.vertical, 11)
    .contextMenu {
      Button("Copy commit ID", systemImage: "doc.on.doc") { NativePlatform.copy(commit.oid) }
    }
  }
  private func dayLabel(_ day: Date) -> String {
    if Calendar.current.isDateInToday(day) { return "Today" }
    if Calendar.current.isDateInYesterday(day) { return "Yesterday" }
    return day.formatted(.dateTime.month(.wide).day())
  }
}

/// Code that scrolls sideways as one block. Rows stretch to the widest line or the visible
/// width, whichever is larger, so their tints run edge to edge.
struct PullRequestCodeScroller<Content: View>: View {
  @ViewBuilder let content: Content
  @State private var width: CGFloat = 0
  var body: some View {
    ScrollView(.horizontal, showsIndicators: false) {
      VStack(alignment: .leading, spacing: 0) {
        Color.clear.frame(width: width, height: 0)
        content
      }
    }
    .onGeometryChange(for: CGFloat.self) {
      $0.size.width
    } action: {
      width = $0
    }
  }
}
