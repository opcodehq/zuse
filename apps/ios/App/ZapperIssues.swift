import SwiftUI
import ZuseCore

struct ZapperIssueRow: View {
  let issue: ZapperIssue
  let statusLabel: String
  var body: some View {
    HStack(alignment: .top, spacing: 10) {
      Image(systemName: issue.details.priority.symbol).font(.system(size: 13, weight: .semibold))
        .foregroundStyle(issue.details.priority.tint).frame(width: 20).padding(.top, 2)
        .accessibilityLabel(issue.details.priority.label + " priority")
      VStack(alignment: .leading, spacing: 3) {
        Text(issue.details.title).foregroundStyle(.primary).lineLimit(2)
        HStack(spacing: 10) {
          Text(statusLabel == "Queued" ? issue.code + " · Queued" : issue.code).monospacedDigit()
          if !issue.threads.isEmpty {
            meta("\(issue.threads.count)", "bubble.left.and.bubble.right", "threads")
          }
          if !issue.pullRequests.isEmpty {
            meta("\(issue.pullRequests.count)", "arrow.triangle.pull", "pull requests")
          }
          if let due = issue.details.dueDate { meta(due, "calendar", "due") }
        }
        .font(.caption).foregroundStyle(.secondary).lineLimit(1)
      }
      Spacer(minLength: 4)
      Image(systemName: "chevron.right").font(.system(size: 13, weight: .semibold))
        .foregroundStyle(.tertiary).padding(.top, 3)
    }
    .contentShape(Rectangle())
    .accessibilityElement(children: .combine)
  }
  private func meta(_ text: String, _ symbol: String, _ spoken: String) -> some View {
    HStack(spacing: 3) {
      Image(systemName: symbol).imageScale(.small)
      Text(text).monospacedDigit()
    }
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(text + " " + spoken)
  }
}

/// One issue in full: what it is, who holds it, linked work and its activity, with a reply
/// field that hands an issue waiting on you back to the Zapper.
struct ZapperIssueView: View {
  let workspace: WorkspaceModel
  let zapperID: String
  let issueID: String
  let push: (ZapperRoute) -> Void
  @State private var reply = ""
  @State private var sending = false
  @FocusState private var replyFocused: Bool
  private var store: ZapperStore { workspace.zappers }

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

  private func content(_ zapper: Zapper, _ issue: ZapperIssue) -> some View {
    List {
      Section {
        VStack(alignment: .leading, spacing: 8) {
          HStack(spacing: 8) {
            Text(issue.code).font(.footnote.weight(.semibold)).monospacedDigit()
              .foregroundStyle(.secondary)
            Label(zapper.issueStatusLabel(issue), systemImage: issue.details.status.symbol)
              .font(.footnote.weight(.semibold)).foregroundStyle(issue.details.status.tint)
          }
          Text(issue.details.title).font(.title3.weight(.bold)).textSelection(.enabled)
        }
        .listRowBackground(Color.clear).listRowInsets(
          EdgeInsets(top: 8, leading: 4, bottom: 4, trailing: 4))
      }
      if issue.details.status == .needsYou, let question = latestQuestion(issue) {
        Section {
          VStack(alignment: .leading, spacing: 6) {
            Label("\(zapper.config.name) is asking", systemImage: "person.fill.questionmark")
              .font(.footnote.weight(.semibold)).foregroundStyle(.orange)
            Text(question.text).font(.subheadline).textSelection(.enabled)
            Button("Reply") { replyFocused = true }.font(.subheadline.weight(.semibold))
              .buttonStyle(.borderless).tint(AppTheme.accent).padding(.top, 2)
          }
        }
      }
      Section {
        statusPicker(zapper, issue)
        priorityPicker(zapper, issue)
        if let owner = issue.details.currentOwner {
          LabeledContent("Owner", value: ownerLabel(owner, zapper: zapper))
        }
        if let due = issue.details.dueDate { LabeledContent("Due", value: due) }
        if !issue.details.labels.isEmpty {
          LabeledContent("Labels", value: issue.details.labels.joined(separator: ", "))
        }
        LabeledContent("Updated", value: zapperRelativeTime(issue.updatedAt))
      }
      textSection("Description", issue.details.description)
      textSection("Done when", issue.details.doneCriteria)
      textSection("Out of scope", issue.details.outOfScope)
      if !issue.threads.isEmpty {
        Section("Threads") {
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
                if let worker {
                  ZapperThreadMark(rank: store.rank(of: worker))
                } else {
                  Image(systemName: "checkmark.circle").foregroundStyle(.secondary)
                    .frame(width: 16)
                }
                Text(title).foregroundStyle(.primary).lineLimit(1)
                Spacer(minLength: 4)
                Image(systemName: "chevron.right").font(.system(size: 13, weight: .semibold))
                  .foregroundStyle(.tertiary)
              }
              .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(worker?.queued == true)
          }
        }
      }
      if !issue.pullRequests.isEmpty {
        Section("Pull requests") {
          ForEach(issue.pullRequests, id: \.url) { pull in
            if let url = URL(string: pull.url), url.scheme == "https" {
              Link(destination: url) {
                HStack(spacing: 10) {
                  Image(systemName: "arrow.triangle.pull").foregroundStyle(.green).frame(width: 20)
                  VStack(alignment: .leading, spacing: 2) {
                    Text(pull.title.isEmpty ? pullLabel(pull.url) : pull.title)
                      .foregroundStyle(.primary).lineLimit(2)
                    Text(pullLabel(pull.url)).font(.caption).foregroundStyle(.secondary)
                  }
                  Spacer(minLength: 4)
                  Image(systemName: "arrow.up.right").font(.footnote).foregroundStyle(.tertiary)
                }
              }
            }
          }
        }
      }
      if !issue.activity.isEmpty {
        Section("Activity") {
          ForEach(issue.activity.reversed()) { entry in
            ActivityRow(entry: entry, actor: actorName(entry, zapper: zapper))
          }
        }
      }
    }
    .zapperList()
    .navigationTitle(issue.code)
    .compactNavigationTitle()
    .safeAreaInset(edge: .bottom) { composer(zapper, issue) }
    .alert(
      "Couldn’t update \(issue.code)", isPresented: failurePresented(zapper.id)
    ) {
      Button("OK") { store.failure.removeValue(forKey: zapper.id) }
    } message: {
      Text(store.failure[zapper.id] ?? "")
    }
  }

  private func statusPicker(_ zapper: Zapper, _ issue: ZapperIssue) -> some View {
    Picker(
      "Status",
      selection: Binding(
        get: { issue.details.status },
        set: { status in
          var details = issue.details
          details.status = status
          // Moving on from a question hands the issue back to the Zapper.
          if details.currentOwner == .user { details.owner = .zapper }
          edit(zapper, issue, details, label: "Status")
        })
    ) {
      if issue.details.status == .needsYou {
        Text(ZapperIssueStatus.needsYou.label).tag(ZapperIssueStatus.needsYou)
      }
      ForEach(ZapperIssueStatus.allCases.filter { $0 != .needsYou }, id: \.self) { status in
        Label(status.label, systemImage: status.symbol).tag(status)
      }
    }
    .pickerStyle(.menu).tint(.secondary)
    .disabled(store.busy[zapper.id] != nil)
  }

  private func priorityPicker(_ zapper: Zapper, _ issue: ZapperIssue) -> some View {
    Picker(
      "Priority",
      selection: Binding(
        get: { issue.details.priority },
        set: { priority in
          var details = issue.details
          details.priority = priority
          edit(zapper, issue, details, label: "Priority")
        })
    ) {
      ForEach(ZapperIssuePriority.allCases, id: \.self) { priority in
        Label(priority.label, systemImage: priority.symbol).tag(priority)
      }
    }
    .pickerStyle(.menu).tint(.secondary)
    .disabled(store.busy[zapper.id] != nil)
  }

  private func edit(
    _ zapper: Zapper, _ issue: ZapperIssue, _ details: ZapperIssueDetails, label: String
  ) {
    guard details != issue.details else { return }
    Task {
      await store.perform(.editIssue(issueId: issue.id, details: details), on: zapper, label: label)
    }
  }

  @ViewBuilder private func textSection(_ title: String, _ text: String) -> some View {
    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    if !trimmed.isEmpty {
      Section(title) { ChatMarkdown(text: trimmed).padding(.vertical, 2) }
    }
  }

  private func composer(_ zapper: Zapper, _ issue: ZapperIssue) -> some View {
    let waiting = issue.details.status == .needsYou
    let canSend = !reply.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !sending
    return HStack(alignment: .bottom, spacing: 8) {
      TextField(
        waiting ? "Answer \(zapper.config.name)…" : "Add a comment…", text: $reply,
        axis: .vertical
      )
      .lineLimit(1...5).focused($replyFocused)
      .padding(.leading, 14).padding(.vertical, 11)
      .accessibilityIdentifier("zapper-issue-comment")
      Button {
        send(zapper, issue)
      } label: {
        ComposerSendGlyph(symbol: "arrow.up", enabled: canSend, busy: sending)
      }
      .buttonStyle(.plain).disabled(!canSend)
      .accessibilityLabel(waiting ? "Send answer" : "Send comment")
      .padding(.trailing, 3).padding(.bottom, 2)
    }
    .composerGlass()
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

  private func latestQuestion(_ issue: ZapperIssue) -> ZapperIssueActivity? {
    issue.activity.last {
      $0.kind == "question" && !$0.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
  }
  private func ownerLabel(_ owner: ZapperIssueOwner, zapper: Zapper) -> String {
    switch owner {
    case .zapper: return zapper.config.name
    case .user: return "You"
    case .thread(let chat):
      return store.title(
        of: chat, fallback: zapper.workers.first { $0.chatId == chat }?.objective ?? "Thread")
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
}

private struct ActivityRow: View {
  let entry: ZapperIssueActivity
  let actor: String
  var body: some View {
    HStack(alignment: .top, spacing: 10) {
      Image(systemName: symbol).font(.system(size: 12, weight: .semibold))
        .foregroundStyle(tint).frame(width: 20).padding(.top, 3)
      VStack(alignment: .leading, spacing: 3) {
        HStack(spacing: 6) {
          Text(actor).font(.footnote.weight(.semibold)).lineLimit(1)
          Text(zapperRelativeTime(entry.createdAt)).font(.caption).foregroundStyle(.tertiary)
        }
        Text(entry.text).font(.subheadline)
          .foregroundStyle(isNote ? .secondary : .primary)
          .lineLimit(isNote ? 3 : 12).textSelection(.enabled)
      }
    }
    .padding(.leading, entry.replyTo == nil ? 0 : 18)
    .padding(.vertical, 2)
  }
  private var isNote: Bool { !["comment", "question"].contains(entry.kind) }
  private var symbol: String {
    switch entry.kind {
    case "comment": return entry.replyTo == nil ? "text.bubble" : "arrowshape.turn.up.left"
    case "question": return "questionmark.bubble.fill"
    case "workerReport": return "bubble.left.and.text.bubble.right"
    case "thread": return "bubble.left.and.bubble.right"
    case "pullRequest": return "arrow.triangle.pull"
    case "closed": return "checkmark.seal"
    case "created": return "plus.circle"
    default: return "pencil"
    }
  }
  private var tint: Color {
    switch entry.kind {
    case "question": return .orange
    case "pullRequest", "closed": return .green
    default: return .secondary
    }
  }
}
