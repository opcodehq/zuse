import SwiftUI
import ZuseCore

/// Every check on the pull request's head, failing first; each opens its run.
struct PullRequestChecksView: View {
  let detail: PullRequestDetail
  var body: some View {
    let checks = PullRequestChecks.ordered(detail.checks)
    let counts = PullRequestChecks(detail.checks)
    ScrollView {
      VStack(alignment: .leading, spacing: 16) {
        HStack(spacing: 12) {
          PullRequestCheckIcon(
            outcome: counts.failed > 0 ? .failed : counts.pending > 0 ? .pending : .passed,
            size: 28)
          VStack(alignment: .leading, spacing: 2) {
            Text(headline(counts)).font(.system(size: 20, weight: .semibold))
            Text(PullRequestChecks.summary(detail)).font(.system(size: 15))
              .foregroundStyle(.secondary)
          }
        }
        .padding(.horizontal, 4).padding(.top, 6)
        if counts.total < detail.ci.totalCount {
          Text("Some check details are unavailable.").font(.footnote).foregroundStyle(.secondary)
            .padding(.horizontal, 4)
        }
        PullRequestCard(padding: 0) {
          VStack(spacing: 0) {
            ForEach(Array(checks.enumerated()), id: \.element.id) { index, check in
              if index > 0 { Divider().padding(.leading, 50) }
              row(check)
            }
          }
        }
      }
      .padding(16)
      .frame(maxWidth: 760).frame(maxWidth: .infinity)
    }
    .background(AppTheme.surface)
    .navigationTitle("Checks")
    .compactNavigationTitle()
  }

  private func headline(_ counts: PullRequestChecks) -> String {
    if counts.failed > 0 {
      return counts.failed == 1 ? "1 check failing" : "\(counts.failed) checks failing"
    }
    if counts.pending > 0 { return "Checks running" }
    return "All checks passed"
  }

  @ViewBuilder private func row(_ check: PullRequestCheck) -> some View {
    let content = HStack(spacing: 12) {
      PullRequestCheckIcon(outcome: check.outcome, size: 18).frame(width: 24)
      VStack(alignment: .leading, spacing: 2) {
        Text(check.title).font(.system(size: 16)).foregroundStyle(.primary).lineLimit(2)
        Text(subtitle(check)).font(.system(size: 13)).foregroundStyle(.secondary).lineLimit(1)
      }
      Spacer(minLength: 6)
      if check.link != nil {
        Image(systemName: "arrow.up.right").font(.system(size: 12, weight: .semibold))
          .foregroundStyle(.tertiary)
      }
    }
    .padding(.horizontal, 14).padding(.vertical, 12).contentShape(Rectangle())
    if let url = check.link {
      Link(destination: url) { content }.buttonStyle(.plain)
    } else {
      content
    }
  }

  private func subtitle(_ check: PullRequestCheck) -> String {
    var parts = [PullRequestChecks.label(check.outcome, raw: check.rawStatus)]
    if let start = PullRequestDate.parse(check.startedAt),
      let end = PullRequestDate.parse(check.completedAt), end > start
    {
      let seconds = Int(end.timeIntervalSince(start))
      parts.append(
        seconds < 60
          ? "\(seconds)s"
          : seconds % 60 == 0 ? "\(seconds / 60)m" : "\(seconds / 60)m \(seconds % 60)s")
    }
    if !check.name.isEmpty && !check.context.isEmpty { parts.append(check.context) }
    return parts.joined(separator: " · ")
  }
}

struct PullRequestCheckIcon: View {
  let outcome: PullRequestCheckOutcome
  var size: CGFloat = 15
  var muted = false
  var body: some View {
    Group {
      switch outcome {
      case .passed:
        Image(systemName: muted ? "circle.dashed" : "checkmark.circle")
          .foregroundStyle(muted ? Color.secondary : PullRequestTint.open)
      case .failed:
        Image(systemName: "xmark.circle").foregroundStyle(PullRequestTint.deletion)
      case .pending:
        Image(systemName: "clock").foregroundStyle(PullRequestTint.attention)
      case .skipped:
        Image(systemName: "minus.circle").foregroundStyle(.secondary)
      }
    }
    .font(.system(size: size, weight: .medium))
    .accessibilityHidden(true)
  }
}
