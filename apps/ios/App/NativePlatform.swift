import SwiftUI
import ZuseCore

#if os(macOS)
  import AppKit
#else
  import UIKit
#endif

#if os(macOS)
  typealias NativeImage = NSImage
#else
  typealias NativeImage = UIImage
#endif

@MainActor enum NativePlatform {
  static var deviceName: String {
    #if os(macOS)
      Host.current().localizedName ?? "Mac"
    #else
      UIDevice.current.name
    #endif
  }
  static func copy(_ text: String) {
    #if os(macOS)
      NSPasteboard.general.clearContents()
      NSPasteboard.general.setString(text, forType: .string)
    #else
      UIPasteboard.general.string = text
    #endif
  }
}

@MainActor enum AppTheme {
  #if os(macOS)
    static let background = NSColor(name: nil) { appearance in
      appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        ? NSColor(white: 0.06, alpha: 1)
        : NSColor(red: 247 / 255, green: 247 / 255, blue: 245 / 255, alpha: 1)
    }
    static let surface = Color(nsColor: background)
    static let card = Color(
      nsColor: NSColor(name: nil) { appearance in
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
          ? NSColor(white: 0.12, alpha: 1) : .white
      })
    static let accent = Color(
      nsColor: NSColor(name: nil) { appearance in
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
          ? NSColor(red: 0.55, green: 0.79, blue: 0.13, alpha: 1)
          : NSColor(red: 0.28, green: 0.41, blue: 0, alpha: 1)
      })
  #else
    static let background = UIColor { traits in
      traits.userInterfaceStyle == .dark
        ? UIColor(white: 0.06, alpha: 1)
        : UIColor(red: 247 / 255, green: 247 / 255, blue: 245 / 255, alpha: 1)
    }
    static let surface = Color(uiColor: background)
    static let card = Color(
      uiColor: UIColor { traits in
        traits.userInterfaceStyle == .dark ? UIColor(white: 0.12, alpha: 1) : .white
      })
    static let accent = Color(
      uiColor: UIColor { traits in
        traits.userInterfaceStyle == .dark
          ? UIColor(red: 0.55, green: 0.79, blue: 0.13, alpha: 1)
          : UIColor(red: 0.28, green: 0.41, blue: 0, alpha: 1)
      })
  #endif
  #if os(macOS)
    static func adaptive(light: NSColor, dark: NSColor) -> Color {
      Color(
        nsColor: NSColor(name: nil) {
          $0.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
        })
    }
    private static func rgb(_ red: CGFloat, _ green: CGFloat, _ blue: CGFloat) -> NSColor {
      NSColor(red: red / 255, green: green / 255, blue: blue / 255, alpha: 1)
    }
  #else
    static func adaptive(light: UIColor, dark: UIColor) -> Color {
      Color(uiColor: UIColor { $0.userInterfaceStyle == .dark ? dark : light })
    }
    private static func rgb(_ red: CGFloat, _ green: CGFloat, _ blue: CGFloat) -> UIColor {
      UIColor(red: red / 255, green: green / 255, blue: blue / 255, alpha: 1)
    }
  #endif
  /// Neutral fills: attachment tiles, pills.
  static let bubble = adaptive(light: rgb(234, 234, 230), dark: rgb(38, 38, 40))
  /// The send control: the brand green, deep enough for a white arrow (4.5:1 or better).
  static let userBubble = adaptive(light: rgb(82, 131, 4), dark: rgb(72, 116, 8))
  /// The brand lime everything neon is mixed from.
  static let lime = Color(red: 140 / 255, green: 201 / 255, blue: 33 / 255)
  /// Text on your messages: 13:1 on the light bubble, 13:1 on the dark one.
  static let userBubbleText = adaptive(light: rgb(26, 41, 5), dark: rgb(234, 247, 211))
  /// Labels on the accent: white on the deep light-mode green (6.6:1), near-black on the
  /// bright dark-mode lime (9:1).
  static let onAccent = adaptive(light: rgb(255, 255, 255), dark: rgb(22, 34, 4))
  /// Hairlines and quiet rules in the transcript.
  static let hairline = adaptive(
    light: rgb(23, 23, 22).withAlphaComponent(0.09),
    dark: rgb(255, 255, 255).withAlphaComponent(0.09)
  )
  /// Line counts and diff gutters.
  static let added = adaptive(light: rgb(58, 122, 0), dark: rgb(141, 201, 60))
  static let removed = adaptive(light: rgb(196, 54, 44), dark: rgb(255, 120, 108))
  /// Code cards and expanded tool output.
  static let codeBackground = adaptive(light: rgb(241, 241, 238), dark: rgb(26, 26, 28))
  static let link = adaptive(light: rgb(0, 102, 214), dark: rgb(116, 172, 255))
}

extension View {
  @ViewBuilder func compactNavigationTitle() -> some View {
    #if os(iOS)
      navigationBarTitleDisplayMode(.inline)
    #else
      self
    #endif
  }
  @ViewBuilder func verbatimInput() -> some View {
    #if os(iOS)
      textInputAutocapitalization(.never)
    #else
      self
    #endif
  }
  @ViewBuilder func compactListSections() -> some View {
    #if os(iOS)
      listSectionSpacing(8)
    #else
      self
    #endif
  }
}

extension Image {
  init(nativeImage: NativeImage) {
    #if os(macOS)
      self.init(nsImage: nativeImage)
    #else
      self.init(uiImage: nativeImage)
    #endif
  }
}

extension View {
  /// Inset-grouped settings list shared by every settings page.
  @ViewBuilder func settingsList() -> some View {
    #if os(iOS)
      listStyle(.insetGrouped).listSectionSpacing(.compact)
        .environment(\.defaultMinListRowHeight, 44)
        .contentMargins(.top, 4, for: .scrollContent)
        .scrollContentBackground(.hidden).background(AppTheme.surface)
    #else
      formStyle(.grouped).scrollContentBackground(.hidden).background(AppTheme.surface)
    #endif
  }
}

struct SettingsIcon: View {
  let symbol: String
  var brand = false
  var body: some View {
    Image(systemName: symbol).font(.system(size: 14, weight: .semibold))
      .foregroundStyle(brand ? AppTheme.onAccent : Color.primary)
      .frame(width: 28, height: 28)
      .background(
        brand ? AppTheme.accent : Color.primary.opacity(0.09),
        in: RoundedRectangle(cornerRadius: 7, style: .continuous)
      )
      .accessibilityHidden(true)
  }
}

struct SettingsRow: View {
  let title: String
  var subtitle: String? = nil
  let icon: String
  var value: String? = nil
  var brand = false
  var destructive = false
  /// Nil lets explanatory subtitles wrap fully.
  var subtitleLines: Int? = 2
  var body: some View {
    HStack(spacing: 12) {
      SettingsIcon(symbol: icon, brand: brand)
      VStack(alignment: .leading, spacing: 1) {
        Text(title).foregroundStyle(destructive ? Color.red : Color.primary)
        if let subtitle {
          Text(subtitle).font(.footnote).foregroundStyle(.secondary).lineLimit(subtitleLines)
            .fixedSize(horizontal: false, vertical: subtitleLines == nil)
        }
      }
      Spacer(minLength: 8)
      if let value { Text(value).foregroundStyle(.secondary).lineLimit(1) }
    }.contentShape(Rectangle())
  }
}

extension ToolbarItemPlacement {
  /// Trailing, non-prominent slot for a sheet's close control.
  static var sheetClose: ToolbarItemPlacement {
    #if os(iOS)
      .topBarTrailing
    #else
      .cancellationAction
    #endif
  }
}

/// Close control for sheets: native glass close button on current systems.
struct SheetCloseButton: View {
  let action: () -> Void
  var body: some View {
    Group {
      if #available(iOS 26, macOS 26, *) {
        Button(role: .close, action: action)
      } else {
        Button("Done", action: action)
      }
    }.tint(.primary)
  }
}

struct BrandTitle: View {
  var body: some View {
    HStack(spacing: 8) {
      Image("brand-mark").resizable().scaledToFill().frame(width: 26, height: 26)
        .clipShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
      Text("Zuse").font(.system(size: 18, weight: .bold))
    }.accessibilityElement(children: .combine).accessibilityAddTraits(.isHeader)
  }
}

extension View {
  @ViewBuilder func nativeSheetSize(width: CGFloat = 480, height: CGFloat = 560) -> some View {
    #if os(macOS)
      frame(minWidth: width, idealWidth: width, minHeight: height, idealHeight: height)
    #else
      presentationSizing(.form)
    #endif
  }
}

extension View {
  @ViewBuilder func nativeWindowMinimum() -> some View {
    #if os(macOS)
      frame(minWidth: 760, minHeight: 520)
    #else
      self
    #endif
  }
}

extension Notification.Name {
  static let nativeNewChat = Notification.Name("sh.zuse.native.new-chat")
  static let nativeNewCloudChat = Notification.Name("sh.zuse.native.new-cloud-chat")
  static let nativeSettings = Notification.Name("sh.zuse.native.settings")
}

extension View {
  /// The one prominent action on a page: a capsule of the brand accent with a label that stays
  /// readable on it in both appearances.
  func brandProminentButton() -> some View { buttonStyle(BrandProminentButtonStyle()) }
}

private struct BrandProminentButtonStyle: ButtonStyle {
  @Environment(\.isEnabled) private var enabled
  @Environment(\.controlSize) private var controlSize
  func makeBody(configuration: Configuration) -> some View {
    configuration.label
      .font(.body.weight(.semibold))
      .foregroundStyle(AppTheme.onAccent)
      .padding(.horizontal, 20)
      .frame(minHeight: controlSize == .large ? 50 : controlSize == .regular ? 40 : 34)
      .background(AppTheme.accent, in: Capsule())
      .opacity(enabled ? (configuration.isPressed ? 0.82 : 1) : 0.45)
      .contentShape(Capsule())
      .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
  }
}

extension View {
  @ViewBuilder func floatingGlass(radius: CGFloat = 24, clear: Bool = false) -> some View {
    let shape = RoundedRectangle(cornerRadius: radius, style: .continuous)
    if #available(iOS 26, macOS 26, *) {
      glassEffect(clear ? .clear : .regular, in: shape)
    } else {
      background(clear ? .ultraThinMaterial : .regularMaterial, in: shape)

    }
  }
}

private struct ComposerGlass: ViewModifier {
  let radius: CGFloat
  @ViewBuilder func body(content: Content) -> some View {
    let shape = RoundedRectangle(cornerRadius: radius, style: .continuous)
    if #available(iOS 26, macOS 26, *) {
      // Regular glass keeps the transcript scrolling underneath from showing through the text.
      content.glassEffect(.regular.interactive(), in: shape)
    } else {
      content.background(.regularMaterial, in: shape)
    }
  }
}
extension View {
  func composerGlass(radius: CGFloat = 22) -> some View { modifier(ComposerGlass(radius: radius)) }
}

/// Accent send/stop control shared by every composer.
struct ComposerSendGlyph: View {
  var symbol = "arrow.up"
  let enabled: Bool
  let busy: Bool
  var body: some View {
    Group {
      if busy {
        ProgressView().tint(.white)
      } else {
        Image(systemName: symbol)
          .font(.system(size: symbol == "stop.fill" ? 12 : 15, weight: .bold))
      }
    }
    .foregroundStyle(enabled ? AnyShapeStyle(.white) : AnyShapeStyle(.secondary))
    .frame(width: 32, height: 32)
    .background(enabled ? AppTheme.userBubble : Color.secondary.opacity(0.16), in: Circle())
    .frame(width: 44, height: 44).contentShape(Rectangle())
  }
}

struct ProviderMark: View {
  let providerID: String
  var size: CGFloat = 16
  private var asset: String? {
    let id = providerID.lowercased()
    if id == "preview" || id.contains("openai") || id.contains("codex") { return "provider-codex" }
    if id.contains("anthropic") || id.contains("claude") { return "provider-claude" }
    if id.contains("google") || id.contains("gemini") { return "provider-gemini" }
    return nil
  }
  var body: some View {
    Group {
      if let asset {
        Image(asset).renderingMode(.template).resizable().scaledToFit()
      } else {
        Image(systemName: "cpu").resizable().scaledToFit()
      }
    }.frame(width: size, height: size).accessibilityHidden(true)
  }
}

struct ModelPickerButton: View {
  let catalog: [ModelProvider]
  let providerID: String
  let modelID: String
  var lockedProvider: String? = nil
  var sample = false
  var allowsReasoning = true
  var modelOptions: Binding<[String: String]> = .constant([:])
  let select: (String, String) -> Void
  @State private var presented = false
  private var currentProvider: ModelProvider? { catalog.first { $0.id == providerID } }
  var body: some View {
    Button {
      presented = true
    } label: {
      HStack(spacing: 5) {
        ProviderMark(providerID: providerID, size: 15)
        Text(
          catalog.first { $0.id == providerID }?.models.first { $0.id == modelID }?.label
            // Until the catalog loads, name the model as the transcript does ("Sonnet 4.5").
            ?? AgentLabel.label(
              .object(["harness": .string(providerID), "model": .string(modelID)]))
        )
        .lineLimit(1).truncationMode(.middle)
      }.font(.system(size: 15, weight: .medium)).foregroundStyle(.secondary)
        .padding(.horizontal, 6).frame(minHeight: 36)
    }.buttonStyle(.plain).accessibilityLabel("Current model: \(modelID)")
      #if DEBUG
        .task {
          if ProcessInfo.processInfo.arguments.contains("--model-sheet-preview") {
            presented = true
          }
        }
      #endif
      .sheet(isPresented: $presented) {
        ModelSettingsSheet(
          catalog: catalog, providerID: providerID, modelID: modelID,
          lockedProvider: lockedProvider, sample: sample, allowsReasoning: allowsReasoning,
          modelOptions: modelOptions,
          select: select)
      }
  }
}

private struct ModelSettingsSheet: View {
  let catalog: [ModelProvider]
  let lockedProvider: String?
  let sample: Bool
  let allowsReasoning: Bool
  let select: (String, String) -> Void
  @Environment(\.dismiss) private var dismiss
  @Binding private var modelOptions: [String: String]
  @State private var provider: String
  @State private var model: String
  init(
    catalog: [ModelProvider], providerID: String, modelID: String,
    lockedProvider: String?, sample: Bool, allowsReasoning: Bool,
    modelOptions: Binding<[String: String]>,
    select: @escaping (String, String) -> Void
  ) {
    self.catalog = catalog
    self.lockedProvider = lockedProvider
    self.sample = sample
    self.allowsReasoning = allowsReasoning
    self.select = select
    _modelOptions = modelOptions
    _provider = State(initialValue: providerID)
    _model = State(initialValue: modelID)
  }
  private var reasoning: ModelProvider.Model.OptionDescriptor? {
    guard allowsReasoning else { return nil }
    return catalog.first { $0.id == provider }?.models.first { $0.id == model }?.reasoning
  }
  private var effortIndex: Int {
    guard let descriptor = reasoning, let levels = descriptor.options else { return 0 }
    let selected = modelOptions[descriptor.id] ?? descriptor.defaultId
    return levels.firstIndex { $0.id == selected } ?? 0
  }
  var body: some View {
    NavigationStack {
      List {
        Section {
          #if os(iOS)
            modelMenuRow(
              "Provider", id: "model-provider-menu",
              value: catalog.first { $0.id == provider }?.name.replacingOccurrences(
                of: " · sample", with: "") ?? provider,
              choices: catalog.filter { lockedProvider == nil || $0.id == lockedProvider }.map {
                ($0.id, $0.name.replacingOccurrences(of: " · sample", with: ""))
              }, selected: provider
            ) { id in
              provider = id
            }.disabled(lockedProvider != nil)
            modelMenuRow(
              "Model", id: "model-model-menu",
              value: catalog.first { $0.id == provider }?.models.first { $0.id == model }?.label
                ?? model,
              choices: (catalog.first { $0.id == provider }?.models ?? []).map {
                ($0.id, $0.label)
              }, selected: model
            ) { model = $0 }
          #else
            Picker("Provider", selection: $provider) {
              ForEach(catalog) { Text($0.name).tag($0.id) }
            }.disabled(lockedProvider != nil)
            Picker("Model", selection: $model) {
              ForEach(catalog.first { $0.id == provider }?.models ?? []) {
                Text($0.label).tag($0.id)
              }
            }
          #endif
          if let descriptor = reasoning, let levels = descriptor.options, levels.count > 1 {
            VStack(alignment: .leading, spacing: 8) {
              HStack(spacing: 8) {
                Image(systemName: "brain").font(.system(size: 15)).foregroundStyle(.secondary)
                Text("Reasoning").font(.system(size: 16))
                Spacer()
                Text(levels[effortIndex].label).font(.system(size: 15, weight: .medium))
                  .foregroundStyle(AppTheme.accent)
              }
              Slider(
                value: Binding(
                  get: { Double(effortIndex) },
                  set: { index in
                    modelOptions[descriptor.id] = levels[Int(index.rounded())].id
                  }), in: 0...Double(levels.count - 1), step: 1
              )
              .frame(height: 40)
              .tint(AppTheme.accent).accessibilityLabel("Reasoning effort")
              .accessibilityValue(levels[effortIndex].label)
              HStack {
                ForEach(Array(levels.enumerated()), id: \.element.id) { index, level in
                  if index > 0 { Spacer(minLength: 0) }
                  Text(level.label).font(.system(size: 12))
                    .foregroundStyle(index == effortIndex ? Color.primary : Color.secondary)
                }
              }
            }.padding(.vertical, 14)
              .listRowInsets(EdgeInsets(top: 0, leading: 16, bottom: 0, trailing: 16))
              .alignmentGuide(.listRowSeparatorLeading) { _ in -16 }
              .alignmentGuide(.listRowSeparatorTrailing) { $0.width + 16 }
          }
        } footer: {
          if sample { Text("Sample models for preview") }
        }
      }.settingsList()
        .navigationTitle("Model settings").compactNavigationTitle()
        .toolbar { ToolbarItem(placement: .sheetClose) { SheetCloseButton { dismiss() } } }
        .onChange(of: provider) { _, id in
          guard let item = catalog.first(where: { $0.id == id }) else { return }
          model = item.defaultModel
          select(id, model)
        }
        .onChange(of: model) { _, id in
          modelOptions = [:]
          if let descriptor = reasoning,
            let value = descriptor.defaultId ?? descriptor.options?.first?.id
          {
            modelOptions[descriptor.id] = value
          }
          select(provider, id)
        }
    }.presentationDetents([.height(reasoning == nil ? 220 : 330)]).presentationDragIndicator(
      .visible
    )
    .presentationCornerRadius(28)
  }
  #if os(iOS)
    private func modelMenuRow(
      _ title: String, id: String, value: String,
      choices: [(String, String)], selected: String, select: @escaping (String) -> Void
    ) -> some View {
      HStack(spacing: 8) {
        ProviderMark(providerID: provider, size: 16)
        Text(title).font(.system(size: 16))
        Spacer(minLength: 8)
        NativeSelectionMenu(
          title: value, identifier: id, choices: choices, selected: selected, select: select
        )
        .frame(width: 210, height: 48)
      }.listRowInsets(EdgeInsets(top: 0, leading: 16, bottom: 0, trailing: 16))
        .alignmentGuide(.listRowSeparatorLeading) { _ in -16 }
        .alignmentGuide(.listRowSeparatorTrailing) { $0.width + 16 }
    }
  #endif

}

#if os(iOS)
  private struct NativeSelectionMenu: View {
    let title: String
    let identifier: String
    let choices: [(String, String)]
    let selected: String
    let select: (String) -> Void
    @State private var open = false
    var body: some View {
      Button {
        open = true
      } label: {
        HStack(spacing: 6) {
          Text(title).lineLimit(1)
          Image(systemName: "chevron.up.chevron.down").font(.system(size: 11, weight: .medium))
        }.font(.system(size: 16)).foregroundStyle(.secondary)
          .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .trailing).contentShape(
            Rectangle())
      }.buttonStyle(.plain).accessibilityIdentifier(identifier)
        .popover(isPresented: $open, arrowEdge: .bottom) {
          ScrollView {
            VStack(spacing: 0) {
              ForEach(choices, id: \.0) { id, label in
                Button {
                  select(id)
                  open = false
                } label: {
                  HStack {
                    if identifier == "model-provider-menu" {
                      ProviderMark(providerID: id, size: 17)
                    }
                    Text(label).font(.system(size: 16))
                    Spacer()
                    if id == selected {
                      Image(systemName: "checkmark").foregroundStyle(AppTheme.accent)
                    }
                  }.padding(.horizontal, 16).frame(height: 44).contentShape(Rectangle())
                }.buttonStyle(.plain).accessibilityIdentifier("selection-choice-\(id)")
              }
            }.padding(.vertical, 6)
          }.frame(width: 260, height: min(CGFloat(choices.count * 44 + 12), 280))
            .presentationCompactAdaptation(.popover)
        }
    }
  }
#endif

struct ComposerHeightKey: PreferenceKey {
  static let defaultValue: CGFloat = 80
  static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}
