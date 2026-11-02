import SwiftUI
import Textual
import ZuseCore

struct InteractionView: View {
  let interaction: JSONValue
  let workspace: WorkspaceModel
  @State private var expanded = false
  @State private var selected: [Int: Set<Int>] = [:]
  @State private var other: [Int: String] = [:]
  @State private var feedback = ""
  @State private var submitting = false
  @State private var error: String?
  @State private var savedResponse: InteractionResponse?
  @State private var restoring = true
  private var responseMethod: String {
    kind == "Plan" ? "session.plan.respond" : "session.answerQuestion"
  }
  private var kind: String { interaction["_tag"].string ?? "" }
  private var id: String { interaction["id"].string ?? "" }
  private var expired: Bool { interaction["recoveryState"].string != nil }
  var body: some View {
    Button {
      expanded = true
    } label: {
      Label(
        expired ? "Response expired" : kind == "Plan" ? "Review plan" : "Answer agent questions",
        systemImage: kind == "Plan" ? "list.clipboard" : "questionmark.bubble"
      )
      .font(.subheadline.weight(.semibold)).padding(12).frame(maxWidth: .infinity)
    }
    .sheet(isPresented: $expanded) {
      NavigationStack {
        ScrollView {
          VStack(alignment: .leading, spacing: 20) {
            Group {
              if kind == "Question" {
                ForEach(Array(interaction["questions"].array.enumerated()), id: \.offset) {
                  index, question in
                  VStack(alignment: .leading, spacing: 10) {
                    Text(question["question"].string ?? "Question").font(.headline)
                    ForEach(Array(question["options"].array.enumerated()), id: \.offset) {
                      optionIndex, option in
                      Button {
                        var values = selected[index] ?? []
                        if values.contains(optionIndex) {
                          values.remove(optionIndex)
                        } else if question["multiSelect"].bool {
                          values.insert(optionIndex)
                        } else {
                          values = [optionIndex]
                        }
                        selected[index] = values
                      } label: {
                        Label(
                          option.string ?? "",
                          systemImage: selected[index, default: []].contains(optionIndex)
                            ? "checkmark.circle.fill" : "circle"
                        )
                        .frame(maxWidth: .infinity, alignment: .leading)
                      }.buttonStyle(.plain).padding(.vertical, 5)
                    }
                    TextField(
                      "Other answer",
                      text: Binding(get: { other[index] ?? "" }, set: { other[index] = $0 }),
                      axis: .vertical
                    ).textFieldStyle(.roundedBorder)
                  }
                }
              } else {
                ChatMarkdown(text: planText)
                TextField("Feedback (optional)", text: $feedback, axis: .vertical).lineLimit(2...6)
                  .textFieldStyle(.roundedBorder)
              }
            }.disabled(savedResponse != nil || submitting || restoring)
            if expired {
              Label(
                "The agent is no longer accepting this response.",
                systemImage: "clock.badge.exclamationmark"
              ).foregroundStyle(.secondary)
            }
            if let error { Text(error).foregroundStyle(.red) }
            if restoring { ProgressView("Restoring response…") }
            if let savedResponse {
              Text(
                savedResponse.confirmed
                  ? "Response sent. Waiting for the agent to continue."
                  : "Delivery is unconfirmed. Retry sends the response you already chose."
              )
              .font(.caption).foregroundStyle(.secondary)
              if !savedResponse.confirmed {
                Button("Retry saved response") { submit(approve: true) }
                  .buttonStyle(.borderedProminent)
                  .disabled(expired || submitting || workspace.connectionState != "Connected")
              }
            } else {
              HStack {
                Button("Cancel request", role: .destructive) { submit(approve: false) }
                Spacer()
                Button(kind == "Plan" ? "Approve plan" : "Send answers") { submit(approve: true) }
                  .buttonStyle(.borderedProminent).disabled(kind == "Question" && !complete)
              }.disabled(
                expired || submitting || restoring || workspace.connectionState != "Connected")
            }
          }.padding(20)
        }
        .navigationTitle(kind == "Plan" ? "Review plan" : "Agent questions")
        .compactNavigationTitle()
        .toolbar {
          ToolbarItem(placement: .cancellationAction) { Button("Close") { expanded = false } }
        }
      }.nativeSheetSize(width: 520, height: 560)
        .task(id: id) { await restoreResponse() }
    }
  }
  private func restoreResponse() async {
    restoring = true
    defer { restoring = false }
    do {
      savedResponse = try await workspace.savedInteractionResponse(
        method: responseMethod, itemID: id)
      guard let savedResponse else { return }
      if kind == "Plan" {
        feedback = savedResponse.fields["feedback"]?.string ?? ""
      } else {
        selected = [:]
        other = [:]
        for answer in savedResponse.fields["answers"]?.array ?? [] {
          guard let index = answer["questionIndex"].int else { continue }
          selected[index] = Set(answer["selected"].array.compactMap(\.int))
          other[index] = answer["other"].string ?? ""
        }
      }
    } catch { self.error = error.localizedDescription }
  }
  private var complete: Bool {
    !interaction["questions"].array.isEmpty
      && interaction["questions"].array.indices.allSatisfy {
        !(selected[$0] ?? []).isEmpty
          || !(other[$0] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
      }
  }
  private var planText: String {
    let message = workspace.transcript.messages.last {
      $0.content["itemId"].string == id && $0.kind == "tool_use"
    }
    return message?.content["input"]["plan"].string ?? message?.content["input"].pretty
      ?? "Review the plan in the conversation before approving."
  }
  private func submit(approve: Bool) {
    submitting = true
    error = nil
    Task {
      defer { submitting = false }
      do {
        if let savedResponse {
          try await workspace.retryInteractionResponse(savedResponse)
        } else if kind == "Plan" {
          try await workspace.respondToPlan(
            id, outcome: approve ? "approved" : "cancelled", feedback: feedback)
        } else if !approve {
          try await workspace.cancelQuestion(id)
        } else {
          let answers: [JSONValue] = interaction["questions"].array.indices.map { index in
            var value: [String: JSONValue] = [
              "questionIndex": .number(Double(index)),
              "selected": .array((selected[index] ?? []).sorted().map { .number(Double($0)) }),
            ]
            let text = (other[index] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            if !text.isEmpty { value["other"] = .string(text) }
            return .object(value)
          }
          try await workspace.answerQuestion(id, answers: answers)
        }
        expanded = false
      } catch {
        self.error = error.localizedDescription
        await restoreResponse()
      }
    }
  }
}
