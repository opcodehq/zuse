import Charts
import JavaScriptCore
import SwiftUI
import ZuseCore

private actor UISpecParser {
  static let shared = UISpecParser()
  private var context: JSContext?
  func parse(_ spec: String) throws -> JSONValue {
    guard spec.utf8.count <= 128 * 1024 else { throw ClientError("This visual is too large.") }
    if context == nil {
      guard let url = Bundle.main.url(forResource: "UIParser", withExtension: "js"),
        let engine = JSContext()
      else { throw ClientError("Visual parser unavailable.") }
      engine.evaluateScript(try String(contentsOf: url, encoding: .utf8))
      context = engine
    }
    guard let context else { throw ClientError("Visual parser unavailable.") }
    context.exception = nil
    let result = context.objectForKeyedSubscript("parseNativeSpec")?.call(withArguments: [spec])
    guard context.exception == nil, let json = result?.toString(),
      let data = json.data(using: .utf8)
    else {
      throw ClientError(context.exception?.toString() ?? "This visual could not be read.")
    }
    let tree = try JSONDecoder().decode(JSONValue.self, from: data)
    try validateDepth(tree, depth: 0)
    return tree
  }
  private func validateDepth(_ node: JSONValue, depth: Int) throws {
    guard depth < 24 else { throw ClientError("This visual is nested too deeply to display.") }
    for child in node["props"]["children"].array { try validateDepth(child, depth: depth + 1) }
    for tab in node["props"]["tabs"].array { try validateDepth(tab["content"], depth: depth + 1) }
  }

}

struct UISpecView: View {
  let spec: String
  var respond: ((String) async throws -> Void)?
  @State private var root: JSONValue?
  @State private var error: String?
  @State private var values: [String: JSONValue] = [:]
  @State private var submitting = false
  @State private var submitted = false
  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      if let root {
        node(root, depth: 0)
      } else if error == nil {
        ProgressView("Rendering visual…")
      }
      if let error {
        Text(error).font(.caption).foregroundStyle(.red)
        if root == nil {
          DisclosureGroup("View source") {
            Text(spec).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
          }
        }
      }
      if submitted { Label("Response queued", systemImage: "checkmark.circle").font(.caption) }
    }.task(id: spec) {
      do { root = try await UISpecParser.shared.parse(spec) } catch {
        self.error = error.localizedDescription
      }
    }
  }
  private func node(_ value: JSONValue, depth: Int) -> AnyView {
    guard depth < 24 else { return AnyView(Text("Visual nesting limit reached.")) }
    let p = value["props"]
    let type = value["typeName"].string ?? ""
    let children = p["children"].array
    return AnyView(
      VStack(alignment: .leading, spacing: 10) {
        switch type {
        case "Card", "Form":
          if let title = p["title"].string { Text(title).font(.headline) }
          ForEach(Array(children.enumerated()), id: \.offset) { _, child in
            node(child, depth: depth + 1)
          }
          if type == "Form" {
            Button(p["submitLabel"].string ?? "Submit") { submitForm(value) }
              .buttonStyle(.borderedProminent).disabled(respond == nil || submitting || submitted)
          }
        case "Grid":
          LazyVGrid(columns: [GridItem(.adaptive(minimum: 150))], alignment: .leading, spacing: 12)
          {
            ForEach(Array(children.enumerated()), id: \.offset) { _, child in
              node(child, depth: depth + 1)
            }
          }
        case "Text": Text(p["text"].string ?? "").textSelection(.enabled)
        case "Stat":
          Text(p["label"].string ?? "").font(.caption).foregroundStyle(.secondary)
          Text(p["value"].string ?? "").font(.title2.weight(.semibold)).monospacedDigit()
          if let hint = p["hint"].string { Text(hint).font(.caption).foregroundStyle(.secondary) }
        case "KeyValue":
          ForEach(Array(p["pairs"].array.enumerated()), id: \.offset) { _, pair in
            LabeledContent(pair["key"].string ?? "", value: pair["value"].string ?? "")
          }
        case "Table":
          ScrollView(.horizontal) {
            Grid(alignment: .leading, horizontalSpacing: 20, verticalSpacing: 10) {
              GridRow {
                ForEach(Array(p["columns"].array.enumerated()), id: \.offset) { _, cell in
                  Text(cell.string ?? "").bold()
                }
              }
              ForEach(Array(p["rows"].array.enumerated()), id: \.offset) { _, row in
                GridRow {
                  ForEach(Array(row.array.enumerated()), id: \.offset) { _, cell in
                    Text(cell.string ?? "").textSelection(.enabled)
                  }
                }
              }
            }.font(.caption)
          }
        case "Progress":
          ProgressView(value: number(p["percent"]), total: 100) { Text(p["label"].string ?? "") }
        case "Badge", "Callout":
          if let title = p["title"].string { Text(title).font(.headline) }
          Text(p["text"].string ?? p["label"].string ?? "").foregroundStyle(tone(p["tone"].string))
        case "List":
          ForEach(Array(p["items"].array.enumerated()), id: \.offset) { _, item in
            Text("• " + (item.string ?? ""))
          }
        case "Steps", "Flow":
          let items = type == "Steps" ? p["items"].array : p["nodes"].array
          ForEach(Array(items.enumerated()), id: \.offset) { index, item in
            HStack(alignment: .top) {
              Image(
                systemName: type == "Flow"
                  ? "arrow.down.circle"
                  : item["status"].string == "done"
                    ? "checkmark.circle.fill"
                    : item["status"].string == "failed" ? "xmark.circle" : "circle")
              VStack(alignment: .leading, spacing: 4) {
                Text(item["label"].string ?? "Step \(index + 1)").font(.subheadline.weight(.medium))
                if let detail = item["detail"].string {
                  Text(detail).font(.caption).foregroundStyle(.secondary)
                }
              }
            }
          }
          if let caption = p["caption"].string {
            Text(caption).font(.caption).foregroundStyle(.secondary)
          }
        case "Tabs":
          // Disclosure sections keep every tab accessible at narrow widths and large text sizes.
          ForEach(Array(p["tabs"].array.enumerated()), id: \.offset) { _, tab in
            DisclosureGroup(tab["label"].string ?? "Details") {
              node(tab["content"], depth: depth + 1)
            }
          }
        case "BarChart", "LineChart", "DonutChart":
          Text(p["title"].string ?? "").font(.headline)
          Chart(Array(p["points"].array.enumerated()), id: \.offset) { index, point in
            let label = point["label"].string ?? String(index)
            let amount = number(point["value"])
            if type == "BarChart" {
              BarMark(x: .value(p["unit"].string ?? "Value", amount), y: .value("Label", label))
            } else if type == "LineChart" {
              LineMark(x: .value("Order", index), y: .value(p["unit"].string ?? "Value", amount))
              PointMark(x: .value("Order", index), y: .value("Value", amount))
            } else {
              SectorMark(angle: .value("Value", amount), innerRadius: .ratio(0.6)).foregroundStyle(
                by: .value("Label", label))
            }
          }.frame(height: 220)
          DisclosureGroup("Chart values") {
            ForEach(Array(p["points"].array.enumerated()), id: \.offset) { _, point in
              LabeledContent(
                point["label"].string ?? "",
                value: number(point["value"]).formatted() + " " + (p["unit"].string ?? ""))
            }
          }.font(.caption)
        case "Input", "TextArea":
          let name = p["name"].string ?? ""
          TextField(
            p["label"].string ?? name,
            text: Binding(
              get: { values[name]?.string ?? p["value"].string ?? "" },
              set: { values[name] = .string(String($0.prefix(type == "Input" ? 2000 : 8000))) }),
            axis: type == "Input" ? .horizontal : .vertical
          ).textFieldStyle(.roundedBorder)
        case "Select", "RadioGroup":
          let name = p["name"].string ?? ""
          Picker(
            p["label"].string ?? name,
            selection: Binding(
              get: {
                values[name]?.string ?? p["value"].string ?? p["options"].array.first?.string ?? ""
              }, set: { values[name] = .string($0) })
          ) {
            ForEach(p["options"].array.compactMap(\.string), id: \.self) { Text($0).tag($0) }
          }
        case "Checkbox":
          let name = p["name"].string ?? ""
          Toggle(
            p["label"].string ?? name,
            isOn: Binding(
              get: { values[name]?.bool ?? p["checked"].bool }, set: { values[name] = .bool($0) }))
        case "Slider":
          let name = p["name"].string ?? ""
          Text(p["label"].string ?? name)
          Slider(
            value: Binding(
              get: { number(values[name] ?? (p["value"] == .null ? p["min"] : p["value"])) },
              set: { values[name] = .number($0) }), in: number(p["min"])...number(p["max"]),
            step: max(0.001, number(p["step"] == .null ? .number(1) : p["step"])))
        case "FollowUps":
          ForEach(Array(p["items"].array.enumerated()), id: \.offset) { _, item in
            Button(item["label"].string ?? "Continue") { submit(item["prompt"].string ?? "") }
              .disabled(respond == nil || submitting || submitted)
          }
        default: Text("Unsupported visual component: \(type)").font(.caption)
        }
      }.frame(maxWidth: .infinity, alignment: .leading))
  }
  private func number(_ value: JSONValue) -> Double {
    if case .number(let n) = value { return n }
    return 0
  }
  private func tone(_ value: String?) -> Color {
    value == "bad" ? .red : value == "warn" ? .orange : value == "good" ? .green : .primary
  }
  private func submitForm(_ form: JSONValue) {
    var answers: [String: JSONValue] = [:]
    func collect(_ node: JSONValue) {
      let p = node["props"]
      if let name = p["name"].string {
        let fallback: JSONValue
        switch node["typeName"].string {
        case "Checkbox": fallback = .bool(p["checked"].bool)
        case "Slider": fallback = p["value"] == .null ? p["min"] : p["value"]
        case "Select", "RadioGroup":
          fallback = p["value"] == .null ? p["options"].array.first ?? .string("") : p["value"]
        default: fallback = p["value"] == .null ? .string("") : p["value"]
        }
        answers[name] = values[name] ?? fallback
      }
      for child in p["children"].array { collect(child) }
      for tab in p["tabs"].array { collect(tab["content"]) }
    }
    collect(form)
    submit("Form response:\n" + JSONValue.object(answers).pretty)
  }
  private func submit(_ text: String) {
    guard let respond, !submitting, !submitted else { return }
    submitting = true
    Task {
      defer { submitting = false }
      do {
        try await respond(text)
        submitted = true
      } catch { self.error = error.localizedDescription }
    }
  }
}
