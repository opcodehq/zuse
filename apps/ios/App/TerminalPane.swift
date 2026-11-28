import SwiftUI
import ZuseCore

#if os(iOS)
  import UIKit
#else
  import AppKit
#endif

/// One shell on the chat's computer: opens it (or reattaches to it), follows its output from
/// the last item seen, and sends typing in order.
@MainActor @Observable final class TerminalSessionModel {
  enum Phase: Equatable {
    case idle, opening, live
    case exited(Int)
    case failed(String)
  }

  let chatID: String
  /// The screen as drawn. Output lands in `working` and is published at most every frame.
  private(set) var screen = TerminalScreen(cols: 60, rows: 20)
  private(set) var phase = Phase.idle
  private(set) var shell: String?
  /// Sticky modifiers from the key bar; they apply to the next key.
  var control = false
  var option = false
  @ObservationIgnored var onOpened: ((TerminalSessionInfo) -> Void)?
  @ObservationIgnored private(set) var terminalID: String?
  @ObservationIgnored private var working = TerminalScreen(cols: 60, rows: 20)
  @ObservationIgnored private var rpc: RPCClient?
  @ObservationIgnored private var stream: Task<Void, Never>?
  @ObservationIgnored private var writes: Task<Void, Never>?
  @ObservationIgnored private var resizing: Task<Void, Never>?
  @ObservationIgnored private var publishing: Task<Void, Never>?
  @ObservationIgnored private var lastSeq: UInt64 = 0
  @ObservationIgnored private var sized = false
  @ObservationIgnored private var preview = false

  init(chatID: String, terminalID: String?) {
    self.chatID = chatID
    self.terminalID = terminalID
  }

  /// `zsh` for a shell the computer reports as `zsh` or a build-stamped `zsh-1a2b3c4d`.
  static func label(_ session: TerminalSessionInfo) -> String {
    let parts = session.shell.split(separator: "-")
    if parts.count > 1, let last = parts.last, last.count >= 8,
      last.allSatisfy({ $0.isHexDigit })
    {
      return parts.dropLast().joined(separator: "-")
    }
    return session.shell.isEmpty ? "Terminal" : session.shell
  }

  // MARK: Connection

  /// Follows the shell over this connection. A new connection resumes after the last output.
  func attach(_ rpc: RPCClient?) {
    guard let rpc, !preview else { return }
    if self.rpc === rpc, stream != nil { return }
    stream?.cancel()
    self.rpc = rpc
    stream = Task { [weak self] in await self?.run(rpc) }
  }

  private func run(_ rpc: RPCClient) async {
    let client = TerminalClient(rpc: rpc)
    // Wait for the first layout so the shell starts at the size it is drawn at.
    for _ in 0..<20 where !sized { try? await Task.sleep(for: .milliseconds(50)) }
    if terminalID == nil {
      phase = .opening
      do {
        let session = try await client.open(chatID: chatID, cols: working.cols, rows: working.rows)
        guard !Task.isCancelled else {
          try? await client.close(session.id)
          return
        }
        terminalID = session.id
        shell = Self.label(session)
        onOpened?(session)
      } catch {
        if !Task.isCancelled { phase = .failed(error.localizedDescription) }
        return
      }
    }
    guard let id = terminalID else { return }
    try? await client.resize(id, cols: working.cols, rows: working.rows)
    var attempt = 0
    while !Task.isCancelled {
      do {
        let events = try await client.subscribe(id, afterSeq: lastSeq)
        if phase != .live { phase = .live }
        for try await value in events {
          guard let event = TerminalStreamEvent(value), event.seq > lastSeq else { continue }
          attempt = 0
          lastSeq = event.seq
          switch event {
          case .data(_, let bytes):
            let reply = working.feed(bytes)
            schedulePublish()
            if !reply.isEmpty { send(reply) }
          case .exit(_, let code, _):
            publish()
            phase = .exited(code)
            return
          }
        }
      } catch is CancellationError {
        return
      } catch {
        if Task.isCancelled { return }
        let message = error.localizedDescription.lowercased()
        if message.contains("not found") || message.contains("unknown terminal")
          || message.contains("no such terminal")
        {
          phase = .failed("This terminal has ended.")
          return
        }
        attempt += 1
        if attempt > 6 {
          phase = .failed(error.localizedDescription)
          return
        }
      }
      // The stream dropped without an exit: resume after the last output.
      try? await Task.sleep(for: .seconds(min(8, 1 << min(attempt, 3))))
    }
  }

  /// Draws at most once a frame, so a burst of output doesn't redraw every chunk.
  private func schedulePublish() {
    guard publishing == nil else { return }
    publishing = Task { [weak self] in
      try? await Task.sleep(for: .milliseconds(30))
      self?.publish()
    }
  }

  private func publish() {
    publishing?.cancel()
    publishing = nil
    screen = working
  }

  /// Opens a fresh shell in place of one that ended.
  func restart() {
    stream?.cancel()
    stream = nil
    terminalID = nil
    lastSeq = 0
    working = TerminalScreen(cols: working.cols, rows: working.rows)
    publish()
    phase = .idle
    let rpc = self.rpc
    self.rpc = nil
    attach(rpc)
  }

  /// Ends the shell on the computer.
  func close() {
    stream?.cancel()
    stream = nil
    resizing?.cancel()
    if case .exited = phase { return }
    guard let id = terminalID, let rpc else { return }
    Task { try? await TerminalClient(rpc: rpc).close(id) }
  }

  // MARK: Size

  /// Fits the screen to the space it is drawn in and tells the shell.
  func fit(cols: Int, rows: Int) {
    let cols = max(20, cols)
    let rows = max(4, rows)
    sized = true
    guard cols != working.cols || rows != working.rows else { return }
    working.resize(cols: cols, rows: rows)
    publish()
    resizing?.cancel()
    guard let rpc, let id = terminalID else { return }
    resizing = Task {
      try? await Task.sleep(for: .milliseconds(150))
      guard !Task.isCancelled else { return }
      try? await TerminalClient(rpc: rpc).resize(id, cols: cols, rows: rows)
    }
  }

  // MARK: Input

  /// Typed text, with the key bar's sticky Control and Option applied to its first character.
  func type(_ text: String) {
    let bytes = TerminalInput.bytes(text, control: control, option: option)
    control = false
    option = false
    send(bytes)
  }

  func press(_ key: TerminalKey) {
    var bytes = key.bytes(applicationCursor: working.applicationCursorKeys)
    if option { bytes.insert(0x1B, at: 0) }
    control = false
    option = false
    send(bytes)
  }

  func paste(_ text: String) {
    send(TerminalInput.paste(text, bracketed: working.bracketedPaste))
  }

  /// Writes in the order typed; keys pressed while the shell is still opening wait for it.
  func send(_ bytes: [UInt8]) {
    guard !bytes.isEmpty else { return }
    if preview {
      echoPreview(bytes)
      return
    }
    guard let rpc else { return }
    let previous = writes
    writes = Task { [weak self] in
      await previous?.value
      for _ in 0..<200 {
        if self?.terminalID != nil || Task.isCancelled { break }
        try? await Task.sleep(for: .milliseconds(50))
      }
      guard let id = self?.terminalID else { return }
      try? await TerminalClient(rpc: rpc).write(id, bytes: bytes)
    }
  }

  // MARK: Preview

  #if DEBUG
    /// A sample session for offline screenshots: a prompt, a listing and a test run.
    func loadPreview() {
      guard !preview else { return }
      preview = true
      shell = "zsh"
      phase = .live
      let esc = "\u{1B}"
      let prompt =
        "\(esc)[1;36m~/atlas\(esc)[0m on \(esc)[1;35m main\(esc)[0m\r\n\(esc)[1;32m❯\(esc)[0m "
      let output =
        prompt + "ls\r\n"
        + "\(esc)[1;34mapps\(esc)[0m  \(esc)[1;34mcrates\(esc)[0m  \(esc)[1;34mdocs\(esc)[0m  "
        + "Cargo.lock  Cargo.toml  README.md\r\n"
        + prompt + "git status --short\r\n"
        + "\(esc)[32mM\(esc)[0m  apps/ios/App/ChatPanes.swift\r\n"
        + "\(esc)[31m??\(esc)[0m apps/ios/App/TerminalPane.swift\r\n"
        + prompt + "cargo test -p atlas-sync\r\n"
        + "\(esc)[1;32m   Compiling\(esc)[0m atlas-sync v0.4.2\r\n"
        + "\(esc)[1;32m    Finished\(esc)[0m test profile in 3.18s\r\n"
        + "running 12 tests\r\n"
        + "test resumes_after_restart ... \(esc)[32mok\(esc)[0m\r\n"
        + "test watcher_waits_for_snapshot ... \(esc)[32mok\(esc)[0m\r\n"
        + "test result: \(esc)[32mok\(esc)[0m. 12 passed; 0 failed\r\n"
        + prompt
      _ = working.feed(output)
      publish()
    }
  #else
    func loadPreview() {}
  #endif

  private func echoPreview(_ bytes: [UInt8]) {
    var echoed: [UInt8] = []
    for byte in bytes {
      switch byte {
      case 0x0D: echoed += Array("\r\n\u{1B}[1;32m❯\u{1B}[0m ".utf8)
      case 0x7F: echoed += [0x08, 0x20, 0x08]
      case 0x20...0x7E: echoed.append(byte)
      default: break
      }
    }
    _ = working.feed(echoed)
    publish()
  }
}

// MARK: - Drawing

/// Monospaced cell size for the terminal font.
struct TerminalMetrics {
  let size: CGFloat
  let cell: CGSize
  static let inset: CGFloat = 10

  static func make(size: CGFloat) -> TerminalMetrics {
    #if os(iOS)
      let font = UIFont.monospacedSystemFont(ofSize: size, weight: .regular)
      let width = ("M" as NSString).size(withAttributes: [.font: font]).width
      let height = ceil(font.lineHeight)
    #else
      let font = NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
      let width = ("M" as NSString).size(withAttributes: [.font: font]).width
      let height = ceil(font.ascender - font.descender + font.leading)
    #endif
    return TerminalMetrics(size: size, cell: CGSize(width: width, height: height))
  }

  func grid(for size: CGSize) -> (cols: Int, rows: Int) {
    let cols = Int((size.width - Self.inset * 2) / cell.width)
    let rows = Int((size.height - 12) / cell.height)
    return (cols, rows)
  }
}

/// ANSI colours tuned to read on the app's own light and dark surfaces.
@MainActor struct TerminalPalette {
  let dark: Bool

  private static let darkColors: [(Double, Double, Double)] = [
    (0.20, 0.20, 0.22), (1.00, 0.45, 0.42), (0.55, 0.81, 0.38), (0.90, 0.75, 0.42),
    (0.42, 0.68, 1.00), (0.83, 0.54, 0.92), (0.34, 0.83, 0.87), (0.82, 0.82, 0.82),
    (0.45, 0.45, 0.48), (1.00, 0.58, 0.55), (0.66, 0.90, 0.48), (1.00, 0.86, 0.52),
    (0.58, 0.78, 1.00), (0.90, 0.66, 0.98), (0.52, 0.92, 0.95), (1.00, 1.00, 1.00),
  ]
  private static let lightColors: [(Double, Double, Double)] = [
    (0.12, 0.12, 0.12), (0.77, 0.21, 0.17), (0.23, 0.48, 0.00), (0.60, 0.40, 0.00),
    (0.00, 0.40, 0.84), (0.54, 0.25, 0.72), (0.04, 0.50, 0.55), (0.43, 0.43, 0.43),
    (0.35, 0.35, 0.35), (0.85, 0.28, 0.22), (0.30, 0.58, 0.05), (0.70, 0.50, 0.00),
    (0.10, 0.50, 0.92), (0.64, 0.35, 0.82), (0.10, 0.60, 0.65), (0.60, 0.60, 0.60),
  ]

  var foreground: Color { dark ? Color(white: 0.9) : Color(white: 0.1) }
  var background: Color { AppTheme.surface }

  func color(_ color: TerminalColor) -> Color? {
    switch color {
    case .standard: return nil
    case .rgb(let red, let green, let blue):
      return Color(red: Double(red) / 255, green: Double(green) / 255, blue: Double(blue) / 255)
    case .indexed(let index):
      let index = Int(index)
      if index < 16 {
        let (red, green, blue) = (dark ? Self.darkColors : Self.lightColors)[index]
        return Color(red: red, green: green, blue: blue)
      }
      if index < 232 {
        let cube = index - 16
        let levels: [Double] = [0, 95, 135, 175, 215, 255]
        return Color(
          red: levels[cube / 36] / 255, green: levels[(cube / 6) % 6] / 255,
          blue: levels[cube % 6] / 255)
      }
      return Color(white: (8 + Double(index - 232) * 10) / 255)
    }
  }

  func attributed(_ runs: [TerminalRun], size: CGFloat) -> AttributedString {
    var line = AttributedString()
    for run in runs {
      var part = AttributedString(run.text)
      var foreground = color(run.style.foreground) ?? self.foreground
      var background = color(run.style.background)
      if run.style.inverse {
        let swapped = background ?? self.background
        background = foreground
        foreground = swapped
      }
      if run.style.dim { foreground = foreground.opacity(0.6) }
      part.foregroundColor = foreground
      if let background { part.backgroundColor = background }
      var font = Font.system(
        size: size, weight: run.style.bold ? .bold : .regular, design: .monospaced)
      if run.style.italic { font = font.italic() }
      part.font = font
      if run.style.underline { part.underlineStyle = .single }
      line += part
    }
    return line
  }
}

/// A terminal pane: the shell's output, the keyboard with its key bar, and resizing to fit.
struct TerminalPaneView: View {
  let model: TerminalSessionModel
  let rpc: RPCClient?
  let preview: Bool
  @State private var keyboard = false
  @Environment(\.colorScheme) private var scheme
  #if os(macOS)
    @FocusState private var focused: Bool
  #endif
  private var metrics: TerminalMetrics {
    #if os(iOS)
      TerminalMetrics.make(size: UIDevice.current.userInterfaceIdiom == .pad ? 14 : 12.5)
    #else
      TerminalMetrics.make(size: 13)
    #endif
  }

  var body: some View {
    let metrics = self.metrics
    let palette = TerminalPalette(dark: scheme == .dark)
    GeometryReader { geometry in
      ScrollView(.vertical) {
        let screen = model.screen
        LazyVStack(alignment: .leading, spacing: 0) {
          ForEach(screen.lines.indices, id: \.self) { index in
            Text(palette.attributed(screen.runs(line: index, cursor: true), size: metrics.size))
              .lineLimit(1)
              .fixedSize(horizontal: true, vertical: false)
              .frame(height: metrics.cell.height, alignment: .leading)
          }
        }
        .padding(.horizontal, TerminalMetrics.inset).padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
      }
      .defaultScrollAnchor(.bottom)
      .defaultScrollAnchor(.bottom, for: .sizeChanges)
      .scrollIndicators(.hidden)
      .onAppear { fit(geometry.size, metrics) }
      .onChange(of: geometry.size) { _, size in fit(size, metrics) }
    }
    .background(palette.background)
    .contentShape(Rectangle())
    .onTapGesture { focus() }
    #if os(iOS)
      .background {
        TerminalKeyInput(model: model, focused: $keyboard).frame(width: 1, height: 1)
      }
    #else
      .focusable()
      .focused($focused)
      .focusEffectDisabled()
      .onKeyPress(phases: [.down, .repeat]) { press in handle(press) }
      .onPasteCommand(of: [.plainText]) { providers in
        for provider in providers {
          _ = provider.loadObject(ofClass: NSString.self) { text, _ in
            guard let text = text as? String else { return }
            Task { @MainActor in model.paste(text) }
          }
        }
      }
    #endif
    .overlay { status }
    .accessibilityElement(children: .combine)
    .accessibilityLabel("Terminal")
    .accessibilityValue(model.screen.text.split(separator: "\n").suffix(3).joined(separator: "\n"))
    .task(id: rpc.map { ObjectIdentifier($0) }) {
      if preview { model.loadPreview() } else { model.attach(rpc) }
    }
    #if DEBUG
      .task { await applyLaunchInput() }
    #endif
  }

  #if DEBUG
    /// `--pane-keyboard` raises the keyboard and key bar; `--terminal-type=<command>` types a
    /// command and Return once the shell is live, as if typed on the keyboard.
    private func applyLaunchInput() async {
      let arguments = ProcessInfo.processInfo.arguments
      if arguments.contains("--pane-keyboard") {
        try? await Task.sleep(for: .seconds(1.5))
        focus()
      }
      guard
        let command = arguments.first(where: { $0.hasPrefix("--terminal-type=") })?
          .dropFirst(16)
      else { return }
      for _ in 0..<100 where model.phase != .live { try? await Task.sleep(for: .milliseconds(100)) }
      try? await Task.sleep(for: .seconds(1))
      model.type(String(command) + "\n")
    }
  #endif

  private func focus() {
    #if os(iOS)
      keyboard = true
    #else
      focused = true
    #endif
  }

  private func fit(_ size: CGSize, _ metrics: TerminalMetrics) {
    guard size.width > 0, size.height > 0 else { return }
    let grid = metrics.grid(for: size)
    model.fit(cols: grid.cols, rows: grid.rows)
  }

  @ViewBuilder private var status: some View {
    switch model.phase {
    case .opening:
      ProgressView("Opening a shell…").padding(16).floatingGlass(radius: 16)
    case .failed(let message):
      VStack(spacing: 12) {
        Image(systemName: "terminal").font(.system(size: 28)).foregroundStyle(.secondary)
        Text(message).font(.subheadline).multilineTextAlignment(.center)
          .foregroundStyle(.secondary)
        Button("Start a new terminal") { model.restart() }.buttonStyle(.borderedProminent)
          .tint(AppTheme.userBubble)
      }
      .padding(24).frame(maxWidth: 320).floatingGlass(radius: 20)
    case .exited(let code):
      VStack {
        Spacer()
        HStack(spacing: 10) {
          Text(code == 0 ? "The shell exited." : "The shell exited with code \(code).")
            .font(.subheadline)
          Spacer(minLength: 4)
          Button("New terminal") { model.restart() }.fontWeight(.semibold)
        }
        .padding(.horizontal, 16).padding(.vertical, 12).floatingGlass(radius: 18)
        .padding(12)
      }
    case .idle where rpc == nil && !preview:
      Text("Connect to your computer to use its terminal.").font(.subheadline)
        .foregroundStyle(.secondary).padding(16).floatingGlass(radius: 16)
    default:
      EmptyView()
    }
  }

  #if os(macOS)
    private func handle(_ press: KeyPress) -> KeyPress.Result {
      if press.modifiers.contains(.command) { return .ignored }
      let keys: [KeyEquivalent: TerminalKey] = [
        .upArrow: .up, .downArrow: .down, .leftArrow: .left, .rightArrow: .right,
        .escape: .escape, .tab: .tab, .return: .enter, .delete: .backspace,
        .deleteForward: .forwardDelete, .home: .home, .end: .end, .pageUp: .pageUp,
        .pageDown: .pageDown,
      ]
      if let key = keys[press.key] {
        if press.modifiers.contains(.option) { model.option = true }
        model.press(key)
        return .handled
      }
      if press.modifiers.contains(.control), let code = TerminalInput.control(press.key.character) {
        model.send(press.modifiers.contains(.option) ? [0x1B, code] : [code])
        return .handled
      }
      guard !press.characters.isEmpty else { return .ignored }
      model.type(press.characters)
      return .handled
    }
  #endif
}

// MARK: - Keyboard (iOS)

#if os(iOS)
  /// A hidden first responder that takes keyboard input for the terminal, with the key bar as
  /// its accessory so the bar rides on top of the keyboard.
  struct TerminalKeyInput: UIViewRepresentable {
    let model: TerminalSessionModel
    @Binding var focused: Bool

    func makeUIView(context: Context) -> TerminalInputView {
      let view = TerminalInputView()
      view.model = model
      view.onFocus = { focused = $0 }
      return view
    }

    func updateUIView(_ view: TerminalInputView, context: Context) {
      view.model = model
      view.onFocus = { focused = $0 }
      if focused && !view.isFirstResponder {
        DispatchQueue.main.async { _ = view.becomeFirstResponder() }
      } else if !focused && view.isFirstResponder {
        DispatchQueue.main.async { _ = view.resignFirstResponder() }
      }
    }
  }

  final class TerminalInputView: UIView, UIKeyInput {
    var model: TerminalSessionModel?
    var onFocus: ((Bool) -> Void)?
    private lazy var bar: UIHostingController<TerminalKeyBar> = {
      let controller = UIHostingController(
        rootView: TerminalKeyBar(model: model) { [weak self] in
          _ = self?.resignFirstResponder()
        })
      controller.view.backgroundColor = .clear
      controller.view.frame = CGRect(x: 0, y: 0, width: 320, height: 56)
      controller.view.autoresizingMask = [.flexibleWidth]
      controller.sizingOptions = []
      return controller
    }()

    override var canBecomeFirstResponder: Bool { true }
    override var inputAccessoryView: UIView? {
      bar.rootView = TerminalKeyBar(model: model) { [weak self] in
        _ = self?.resignFirstResponder()
      }
      return bar.view
    }
    override func becomeFirstResponder() -> Bool {
      let became = super.becomeFirstResponder()
      if became { onFocus?(true) }
      return became
    }
    override func resignFirstResponder() -> Bool {
      let resigned = super.resignFirstResponder()
      if resigned { onFocus?(false) }
      return resigned
    }

    // UIKeyInput
    var hasText: Bool { true }
    func insertText(_ text: String) { model?.type(text) }
    func deleteBackward() { model?.press(.backspace) }

    // UITextInputTraits: a shell wants exactly what was typed.
    var autocorrectionType: UITextAutocorrectionType = .no
    var autocapitalizationType: UITextAutocapitalizationType = .none
    var spellCheckingType: UITextSpellCheckingType = .no
    var smartQuotesType: UITextSmartQuotesType = .no
    var smartDashesType: UITextSmartDashesType = .no
    var smartInsertDeleteType: UITextSmartInsertDeleteType = .no
    var inlinePredictionType: UITextInlinePredictionType = .no
    var keyboardType: UIKeyboardType = .asciiCapable
    var keyboardAppearance: UIKeyboardAppearance = .default
    var returnKeyType: UIReturnKeyType = .default

    /// A hardware keyboard's arrows, Escape and Control combinations.
    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
      var unhandled = Set<UIPress>()
      for press in presses {
        guard let key = press.key, let model else {
          unhandled.insert(press)
          continue
        }
        let special: [UIKeyboardHIDUsage: TerminalKey] = [
          .keyboardUpArrow: .up, .keyboardDownArrow: .down, .keyboardLeftArrow: .left,
          .keyboardRightArrow: .right, .keyboardEscape: .escape, .keyboardHome: .home,
          .keyboardEnd: .end, .keyboardPageUp: .pageUp, .keyboardPageDown: .pageDown,
          .keyboardDeleteForward: .forwardDelete,
        ]
        if let mapped = special[key.keyCode] {
          if key.modifierFlags.contains(.alternate) { model.option = true }
          model.press(mapped)
        } else if key.modifierFlags.contains(.control),
          let character = key.charactersIgnoringModifiers.first,
          let code = TerminalInput.control(character)
        {
          model.send([code])
        } else {
          unhandled.insert(press)
        }
      }
      if !unhandled.isEmpty { super.pressesBegan(unhandled, with: event) }
    }

    override func paste(_ sender: Any?) {
      if let text = UIPasteboard.general.string { model?.paste(text) }
    }
    override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
      action == #selector(paste(_:)) ? UIPasteboard.general.hasStrings : false
    }
  }

  /// Keys a phone keyboard lacks: arrows, Escape, sticky Control and Option, Tab and the
  /// symbols shells use most.
  struct TerminalKeyBar: View {
    let model: TerminalSessionModel?
    let dismiss: () -> Void

    var body: some View {
      HStack(spacing: 8) {
        ScrollView(.horizontal) {
          HStack(spacing: 6) {
            TerminalArrowPad { model?.press($0) }
            key("esc", label: "Escape") { model?.press(.escape) }
            toggle("ctrl", label: "Control", on: model?.control == true) {
              model?.control.toggle()
            }
            toggle("alt", label: "Option", on: model?.option == true) { model?.option.toggle() }
            key("tab", label: "Tab") { model?.press(.tab) }
            ForEach(["|", "~", "/", "-"], id: \.self) { symbol in
              key(symbol, label: symbol) { model?.type(symbol) }
            }
          }
          .padding(5)
        }
        .scrollIndicators(.hidden)
        .floatingGlass(radius: 25)
        Button(action: dismiss) {
          Image(systemName: "chevron.down").font(.system(size: 15, weight: .semibold))
            .frame(width: 48, height: 48).contentShape(Circle())
        }
        .buttonStyle(.plain)
        .floatingGlass(radius: 24)
        .accessibilityLabel("Hide keyboard")
      }
      .padding(.horizontal, 8).padding(.vertical, 4)
      .frame(height: 56)
    }

    private func key(_ title: String, label: String, action: @escaping () -> Void) -> some View {
      Button(action: action) {
        Text(title).font(.system(size: 15, weight: .medium))
          .frame(minWidth: 38, minHeight: 38).padding(.horizontal, title.count > 1 ? 4 : 0)
          .background(Color.primary.opacity(0.08), in: Capsule())
          .contentShape(Capsule())
      }
      .buttonStyle(.plain)
      .accessibilityLabel(label)
    }

    private func toggle(_ title: String, label: String, on: Bool, action: @escaping () -> Void)
      -> some View
    {
      Button(action: action) {
        Text(title).font(.system(size: 15, weight: .medium))
          .foregroundStyle(on ? AppTheme.onAccent : Color.primary)
          .frame(minWidth: 38, minHeight: 38).padding(.horizontal, 4)
          .background(on ? AppTheme.accent : Color.primary.opacity(0.08), in: Capsule())
          .contentShape(Capsule())
      }
      .buttonStyle(.plain)
      .accessibilityLabel(label)
      .accessibilityAddTraits(on ? .isSelected : [])
    }
  }

  /// Four arrows in one round key: tap the side you want.
  struct TerminalArrowPad: View {
    let press: (TerminalKey) -> Void
    var body: some View {
      ZStack {
        Circle().fill(Color.primary.opacity(0.08))
        ForEach(0..<4) { index in
          Image(systemName: "arrowtriangle.up.fill").font(.system(size: 6))
            .foregroundStyle(.secondary)
            .offset(y: -12)
            .rotationEffect(.degrees(Double(index) * 90))
        }
      }
      .frame(width: 38, height: 38)
      .contentShape(Circle())
      .gesture(
        SpatialTapGesture().onEnded { tap in
          let dx = tap.location.x - 19
          let dy = tap.location.y - 19
          if abs(dx) > abs(dy) {
            press(dx > 0 ? .right : .left)
          } else {
            press(dy > 0 ? .down : .up)
          }
        }
      )
      .accessibilityElement()
      .accessibilityLabel("Arrow keys")
      .accessibilityAdjustableAction { direction in
        press(direction == .increment ? .up : .down)
      }
      .accessibilityActions {
        Button("Left") { press(.left) }
        Button("Right") { press(.right) }
      }
    }
  }
#endif
