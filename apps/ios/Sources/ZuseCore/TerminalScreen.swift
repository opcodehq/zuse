import Foundation

/// A terminal colour: the theme's default, one of the 256 indexed colours, or 24-bit.
public enum TerminalColor: Hashable, Sendable {
  case standard
  case indexed(UInt8)
  case rgb(UInt8, UInt8, UInt8)
}

/// How a cell is drawn (SGR attributes).
public struct TerminalStyle: Hashable, Sendable {
  public var foreground: TerminalColor = .standard
  public var background: TerminalColor = .standard
  public var bold = false
  public var dim = false
  public var italic = false
  public var underline = false
  public var inverse = false
  public init(
    foreground: TerminalColor = .standard, background: TerminalColor = .standard,
    bold: Bool = false, dim: Bool = false, italic: Bool = false, underline: Bool = false,
    inverse: Bool = false
  ) {
    self.foreground = foreground
    self.background = background
    self.bold = bold
    self.dim = dim
    self.italic = italic
    self.underline = underline
    self.inverse = inverse
  }
  public static let plain = TerminalStyle()
}

public struct TerminalCell: Hashable, Sendable {
  public var text: String
  public var style: TerminalStyle
  /// The right half of a double-width character; it draws nothing.
  public var continuation: Bool
  public init(_ text: String, style: TerminalStyle = .plain, continuation: Bool = false) {
    self.text = text
    self.style = style
    self.continuation = continuation
  }
  static func blank(_ style: TerminalStyle) -> TerminalCell {
    TerminalCell(" ", style: TerminalStyle(background: style.background))
  }
}

public struct TerminalLine: Hashable, Sendable {
  public var cells: [TerminalCell] = []
  public init(cells: [TerminalCell] = []) { self.cells = cells }
  public var text: String { cells.filter { !$0.continuation }.map(\.text).joined() }
}

/// A stretch of one line drawn in one style.
public struct TerminalRun: Hashable, Sendable {
  public var text: String
  public var style: TerminalStyle
  public init(text: String, style: TerminalStyle) {
    self.text = text
    self.style = style
  }
}

/// A small VT100/xterm screen: enough of the protocol for shells, prompts and line-oriented
/// tools. It keeps scrollback, SGR colours and attributes, cursor movement and erasing,
/// scroll regions, the alternate screen, and answers cursor and device queries. It doesn't
/// draw box-drawing charsets, mouse reporting or reflow lines on resize.
public struct TerminalScreen: Sendable {
  public private(set) var cols: Int
  public private(set) var rows: Int
  /// Scrollback followed by the screen: the last `rows` lines are the screen.
  public private(set) var lines: [TerminalLine]
  /// Cursor position on the screen, zero-based.
  public private(set) var cursorRow = 0
  public private(set) var cursorCol = 0
  public private(set) var cursorVisible = true
  /// Arrow keys send `ESC O A` rather than `ESC [ A` (DECCKM), as full-screen programs ask.
  public private(set) var applicationCursorKeys = false
  public private(set) var bracketedPaste = false
  public private(set) var title: String?
  public let scrollback: Int

  private var style = TerminalStyle.plain
  private var pendingWrap = false
  private var saved: (row: Int, col: Int, style: TerminalStyle)?
  private var scrollTop = 0
  private var scrollBottom: Int
  private var alternate: (lines: [TerminalLine], row: Int, col: Int)?
  private var state = State.ground
  private var sequence: [UInt8] = []
  private var utf8: [UInt8] = []
  private var replies: [UInt8] = []

  private enum State {
    case ground, escape, skipOne, csi, osc, oscEscape, string, stringEscape
  }

  public init(cols: Int = 80, rows: Int = 24, scrollback: Int = 2000) {
    self.cols = max(2, cols)
    self.rows = max(1, rows)
    self.scrollback = scrollback
    scrollBottom = max(1, rows) - 1
    lines = Array(repeating: TerminalLine(), count: max(1, rows))
  }

  /// Index of the screen's first line in `lines`.
  public var screenTop: Int { lines.count - rows }
  /// The cursor's line in `lines`.
  public var cursorLine: Int { screenTop + cursorRow }
  public var usesAlternateScreen: Bool { alternate != nil }

  // MARK: Input

  /// Feeds program output. Returns bytes the terminal has to send back (query replies).
  public mutating func feed(_ bytes: [UInt8]) -> [UInt8] {
    for byte in bytes { consume(byte) }
    flushUTF8(final: false)
    defer { replies = [] }
    return replies
  }

  public mutating func feed(_ text: String) -> [UInt8] { feed(Array(text.utf8)) }

  private mutating func consume(_ byte: UInt8) {
    switch state {
    case .ground:
      if byte == 0x1B {
        flushUTF8(final: true)
        state = .escape
      } else if byte < 0x20 || byte == 0x7F {
        flushUTF8(final: true)
        control(byte)
      } else {
        utf8.append(byte)
        flushUTF8(final: false)
      }
    case .escape:
      escape(byte)
    case .skipOne:
      state = .ground
    case .csi:
      if byte == 0x1B {
        state = .escape
      } else if byte >= 0x40 && byte <= 0x7E {
        csi(final: byte)
        state = .ground
      } else if byte < 0x20 {
        control(byte)
      } else {
        sequence.append(byte)
        if sequence.count > 64 { state = .ground }
      }
    case .osc:
      if byte == 0x07 {
        osc()
        state = .ground
      } else if byte == 0x1B {
        state = .oscEscape
      } else if sequence.count < 4096 {
        sequence.append(byte)
      }
    case .oscEscape:
      osc()
      state = .ground
      if byte != 0x5C { consume(byte) }
    case .string:
      if byte == 0x1B { state = .stringEscape } else if byte == 0x07 { state = .ground }
    case .stringEscape:
      state = byte == 0x5C ? .ground : .string
    }
  }

  private mutating func escape(_ byte: UInt8) {
    state = .ground
    switch byte {
    case 0x5B:  // [
      sequence = []
      state = .csi
    case 0x5D:  // ]
      sequence = []
      state = .osc
    case 0x50, 0x5F, 0x5E, 0x58:  // P _ ^ X: strings this terminal ignores
      state = .string
    case 0x28, 0x29, 0x2A, 0x2B, 0x23, 0x25, 0x20:  // charset and line-size selections
      state = .skipOne
    case 0x37:  // 7
      saved = (cursorRow, cursorCol, style)
    case 0x38:  // 8
      restoreCursor()
    case 0x44:  // D
      lineFeed()
    case 0x45:  // E
      carriageReturn()
      lineFeed()
    case 0x4D:  // M
      reverseIndex()
    case 0x63:  // c
      reset()
    default:
      break
    }
  }

  private mutating func control(_ byte: UInt8) {
    switch byte {
    case 0x08:
      pendingWrap = false
      if cursorCol > 0 { cursorCol -= 1 }
    case 0x09:
      pendingWrap = false
      cursorCol = min(cols - 1, (cursorCol / 8 + 1) * 8)
    case 0x0A, 0x0B, 0x0C:
      lineFeed()
    case 0x0D:
      carriageReturn()
    default:
      break
    }
  }

  // MARK: Text

  private mutating func flushUTF8(final: Bool) {
    while let lead = utf8.first {
      let length: Int
      switch lead {
      case 0x00..<0x80: length = 1
      case 0xC2..<0xE0: length = 2
      case 0xE0..<0xF0: length = 3
      case 0xF0..<0xF5: length = 4
      default: length = 0
      }
      if length == 0 {
        utf8.removeFirst()
        put("\u{FFFD}")
        continue
      }
      if utf8.count < length {
        // A sequence split across reads waits for the rest, unless a control byte cut it.
        guard final else { return }
        utf8 = []
        put("\u{FFFD}")
        return
      }
      let chunk = Array(utf8.prefix(length))
      utf8.removeFirst(length)
      guard chunk.dropFirst().allSatisfy({ $0 & 0xC0 == 0x80 }),
        let text = String(bytes: chunk, encoding: .utf8), let scalar = text.unicodeScalars.first
      else {
        put("\u{FFFD}")
        continue
      }
      put(scalar)
    }
  }

  private mutating func put(_ scalar: Unicode.Scalar) {
    let category = scalar.properties.generalCategory
    if category == .nonspacingMark || category == .enclosingMark
      || scalar.value == 0x200D || (0xFE00...0xFE0F).contains(scalar.value)
    {
      appendToPrevious(scalar)
      return
    }
    let width = Self.width(of: scalar)
    if pendingWrap || (width == 2 && cursorCol == cols - 1) {
      pendingWrap = false
      carriageReturn()
      lineFeed()
    }
    let index = cursorLine
    write(TerminalCell(String(scalar), style: style), at: cursorCol, line: index)
    if width == 2, cursorCol + 1 < cols {
      write(TerminalCell("", style: style, continuation: true), at: cursorCol + 1, line: index)
    }
    if cursorCol + width >= cols {
      cursorCol = cols - 1
      pendingWrap = true
    } else {
      cursorCol += width
    }
  }

  private mutating func appendToPrevious(_ scalar: Unicode.Scalar) {
    let index = cursorLine
    var column = pendingWrap ? cursorCol : cursorCol - 1
    while column > 0, column < lines[index].cells.count, lines[index].cells[column].continuation {
      column -= 1
    }
    guard column >= 0, column < lines[index].cells.count else { return }
    lines[index].cells[column].text.unicodeScalars.append(scalar)
  }

  private mutating func write(_ cell: TerminalCell, at column: Int, line index: Int) {
    var cells = lines[index].cells
    if cells.count <= column {
      cells.append(
        contentsOf: repeatElement(TerminalCell(" "), count: column - cells.count + 1))
    }
    // Overwriting half of a wide character clears the other half.
    if cells[column].continuation, column > 0 { cells[column - 1] = TerminalCell(" ") }
    if column + 1 < cells.count, cells[column + 1].continuation, !cell.continuation {
      cells[column + 1] = TerminalCell(" ")
    }
    cells[column] = cell
    lines[index].cells = cells
  }

  /// Columns a character takes: two for East Asian wide characters and emoji.
  static func width(of scalar: Unicode.Scalar) -> Int {
    let value = scalar.value
    if scalar.properties.isEmojiPresentation { return 2 }
    switch value {
    case 0x1100...0x115F, 0x2E80...0x303E, 0x3041...0x33FF, 0x3400...0x4DBF, 0x4E00...0x9FFF,
      0xA000...0xA4CF, 0xAC00...0xD7A3, 0xF900...0xFAFF, 0xFE30...0xFE4F, 0xFF00...0xFF60,
      0xFFE0...0xFFE6, 0x20000...0x3FFFD:
      return 2
    default:
      return 1
    }
  }

  // MARK: Movement

  private mutating func carriageReturn() {
    pendingWrap = false
    cursorCol = 0
  }

  private mutating func lineFeed() {
    pendingWrap = false
    if cursorRow == scrollBottom {
      scrollUp(1)
    } else if cursorRow < rows - 1 {
      cursorRow += 1
    }
  }

  private mutating func reverseIndex() {
    pendingWrap = false
    if cursorRow == scrollTop { scrollDown(1) } else if cursorRow > 0 { cursorRow -= 1 }
  }

  private mutating func scrollUp(_ count: Int, feedsScrollback: Bool = true) {
    let count = min(count, scrollBottom - scrollTop + 1)
    guard count > 0 else { return }
    if feedsScrollback && scrollTop == 0 && scrollBottom == rows - 1 && alternate == nil {
      // The whole main screen scrolls: the top lines go to scrollback.
      lines.append(contentsOf: repeatElement(blankLine(), count: count))
      let excess = lines.count - rows - scrollback
      if excess > 0 { lines.removeFirst(excess) }
    } else {
      let top = screenTop + scrollTop
      let bottom = screenTop + scrollBottom
      lines.removeSubrange(top..<(top + count))
      lines.insert(contentsOf: repeatElement(blankLine(), count: count), at: bottom - count + 1)
    }
  }

  private mutating func scrollDown(_ count: Int) {
    let count = min(count, scrollBottom - scrollTop + 1)
    guard count > 0 else { return }
    let top = screenTop + scrollTop
    let bottom = screenTop + scrollBottom
    lines.removeSubrange((bottom - count + 1)...bottom)
    lines.insert(contentsOf: repeatElement(blankLine(), count: count), at: top)
  }

  private func blankLine() -> TerminalLine {
    style.background == .standard
      ? TerminalLine()
      : TerminalLine(cells: Array(repeating: .blank(style), count: cols))
  }

  private mutating func moveTo(row: Int, col: Int) {
    pendingWrap = false
    cursorRow = min(max(0, row), rows - 1)
    cursorCol = min(max(0, col), cols - 1)
  }

  private mutating func restoreCursor() {
    guard let saved else {
      moveTo(row: 0, col: 0)
      return
    }
    moveTo(row: saved.row, col: saved.col)
    style = saved.style
  }

  // MARK: Control sequences

  private mutating func csi(final: UInt8) {
    var body = sequence
    var prefix: UInt8?
    if let first = body.first, first >= 0x3C && first <= 0x3F {
      prefix = first
      body.removeFirst()
    }
    // Intermediate bytes (cursor shape, soft reset and the like) select nothing drawn here.
    if body.contains(where: { $0 >= 0x20 && $0 <= 0x2F }) { return }
    let params = String(decoding: body, as: UTF8.self)
      .split(separator: ";", omittingEmptySubsequences: false)
      .flatMap { $0.split(separator: ":", omittingEmptySubsequences: false) }
      .map { Int($0) }
    func value(_ index: Int, _ fallback: Int = 1) -> Int {
      guard index < params.count, let number = params[index], number > 0 else { return fallback }
      return number
    }
    switch (prefix, final) {
    case (nil, 0x41): moveTo(row: cursorRow - value(0), col: cursorCol)  // A
    case (nil, 0x42), (nil, 0x65): moveTo(row: cursorRow + value(0), col: cursorCol)  // B e
    case (nil, 0x43), (nil, 0x61): moveTo(row: cursorRow, col: cursorCol + value(0))  // C a
    case (nil, 0x44): moveTo(row: cursorRow, col: cursorCol - value(0))  // D
    case (nil, 0x45): moveTo(row: cursorRow + value(0), col: 0)  // E
    case (nil, 0x46): moveTo(row: cursorRow - value(0), col: 0)  // F
    case (nil, 0x47), (nil, 0x60): moveTo(row: cursorRow, col: value(0) - 1)  // G `
    case (nil, 0x48), (nil, 0x66): moveTo(row: value(0) - 1, col: value(1) - 1)  // H f
    case (nil, 0x64): moveTo(row: value(0) - 1, col: cursorCol)  // d
    case (nil, 0x4A): eraseDisplay(value(0, 0))  // J
    case (nil, 0x4B): eraseLine(value(0, 0))  // K
    case (nil, 0x4C): insertLines(value(0))  // L
    case (nil, 0x4D): deleteLines(value(0))  // M
    case (nil, 0x50): deleteCharacters(value(0))  // P
    case (nil, 0x40): insertCharacters(value(0))  // @
    case (nil, 0x58): eraseCharacters(value(0))  // X
    case (nil, 0x53): scrollUp(value(0))  // S
    case (nil, 0x54): scrollDown(value(0))  // T
    case (nil, 0x72):  // r
      let top = value(0) - 1
      let bottom = min(value(1, rows), rows) - 1
      if top < bottom {
        scrollTop = top
        scrollBottom = bottom
      } else {
        scrollTop = 0
        scrollBottom = rows - 1
      }
      moveTo(row: 0, col: 0)
    case (nil, 0x73): saved = (cursorRow, cursorCol, style)  // s
    case (nil, 0x75): restoreCursor()  // u
    case (nil, 0x6D): graphics(params)  // m
    case (nil, 0x6E):  // n
      if value(0, 0) == 5 { replies += Array("\u{1B}[0n".utf8) }
      if value(0, 0) == 6 { replies += Array("\u{1B}[\(cursorRow + 1);\(cursorCol + 1)R".utf8) }
    case (nil, 0x63): replies += Array("\u{1B}[?1;2c".utf8)  // c
    case (0x3E, 0x63): replies += Array("\u{1B}[>0;10;1c".utf8)  // > c
    case (0x3F, 0x68), (0x3F, 0x6C):  // ? h, ? l
      for mode in params.compactMap({ $0 }) { privateMode(mode, on: final == 0x68) }
    default:
      break
    }
  }

  private mutating func privateMode(_ mode: Int, on: Bool) {
    switch mode {
    case 1: applicationCursorKeys = on
    case 25: cursorVisible = on
    case 2004: bracketedPaste = on
    case 47, 1047, 1049:
      if on, alternate == nil {
        if mode == 1049 { saved = (cursorRow, cursorCol, style) }
        alternate = (lines, cursorRow, cursorCol)
        lines = Array(repeating: TerminalLine(), count: rows)
        scrollTop = 0
        scrollBottom = rows - 1
        moveTo(row: 0, col: 0)
      } else if !on, let main = alternate {
        alternate = nil
        lines = main.lines
        normalize()
        scrollTop = 0
        scrollBottom = rows - 1
        moveTo(row: main.row, col: main.col)
        if mode == 1049 { restoreCursor() }
      }
    default:
      break
    }
  }

  private mutating func graphics(_ params: [Int?]) {
    let codes = params.isEmpty ? [0] : params.map { $0 ?? 0 }
    var index = 0
    func extended() -> TerminalColor? {
      guard index + 1 < codes.count else { return nil }
      if codes[index + 1] == 5, index + 2 < codes.count {
        index += 2
        return .indexed(UInt8(clamping: codes[index]))
      }
      if codes[index + 1] == 2, index + 4 < codes.count {
        let color = TerminalColor.rgb(
          UInt8(clamping: codes[index + 2]), UInt8(clamping: codes[index + 3]),
          UInt8(clamping: codes[index + 4]))
        index += 4
        return color
      }
      return nil
    }
    while index < codes.count {
      let code = codes[index]
      switch code {
      case 0: style = .plain
      case 1: style.bold = true
      case 2: style.dim = true
      case 3: style.italic = true
      case 4: style.underline = true
      case 7: style.inverse = true
      case 21, 22:
        style.bold = false
        style.dim = false
      case 23: style.italic = false
      case 24: style.underline = false
      case 27: style.inverse = false
      case 30...37: style.foreground = .indexed(UInt8(code - 30))
      case 38: if let color = extended() { style.foreground = color }
      case 39: style.foreground = .standard
      case 40...47: style.background = .indexed(UInt8(code - 40))
      case 48: if let color = extended() { style.background = color }
      case 49: style.background = .standard
      case 90...97: style.foreground = .indexed(UInt8(code - 90 + 8))
      case 100...107: style.background = .indexed(UInt8(code - 100 + 8))
      default: break
      }
      index += 1
    }
  }

  private mutating func osc() {
    let text = String(decoding: sequence, as: UTF8.self)
    if let separator = text.firstIndex(of: ";") {
      let code = text[..<separator]
      if code == "0" || code == "2" { title = String(text[text.index(after: separator)...]) }
    }
  }

  // MARK: Editing

  private mutating func eraseLine(_ mode: Int) {
    pendingWrap = false
    let index = cursorLine
    var cells = lines[index].cells
    let blank = TerminalCell.blank(style)
    switch mode {
    case 0:
      if cells.count > cursorCol { cells.removeSubrange(cursorCol...) }
      if style.background != .standard {
        cells.append(contentsOf: repeatElement(blank, count: max(0, cols - cells.count)))
      }
    case 1:
      if cells.count <= cursorCol {
        cells.append(
          contentsOf: repeatElement(TerminalCell(" "), count: cursorCol - cells.count + 1))
      }
      for column in 0...cursorCol { cells[column] = blank }
    default:
      cells = style.background == .standard ? [] : Array(repeating: blank, count: cols)
    }
    lines[index].cells = cells
  }

  private mutating func eraseDisplay(_ mode: Int) {
    let top = screenTop
    switch mode {
    case 0:
      eraseLine(0)
      for index in (cursorLine + 1)..<lines.count { lines[index] = blankLine() }
    case 1:
      eraseLine(1)
      for index in top..<cursorLine { lines[index] = blankLine() }
    case 3:
      // Clear scrollback too.
      if alternate == nil { lines.removeFirst(top) }
      for index in 0..<lines.count { lines[index] = blankLine() }
    default:
      if alternate == nil, lines[top..<lines.count].contains(where: { !$0.cells.isEmpty }) {
        // Like xterm, a clear keeps what was on screen in scrollback.
        let used =
          (lines[top..<lines.count].lastIndex(where: { !$0.cells.isEmpty }) ?? top) - top + 1
        lines.append(contentsOf: repeatElement(TerminalLine(), count: used))
        let excess = lines.count - rows - scrollback
        if excess > 0 { lines.removeFirst(excess) }
      }
      for index in screenTop..<lines.count { lines[index] = blankLine() }
    }
  }

  private mutating func insertLines(_ count: Int) {
    guard cursorRow >= scrollTop, cursorRow <= scrollBottom else { return }
    let top = scrollTop
    scrollTop = cursorRow
    scrollDown(count)
    scrollTop = top
    cursorCol = 0
  }

  private mutating func deleteLines(_ count: Int) {
    guard cursorRow >= scrollTop, cursorRow <= scrollBottom else { return }
    let top = scrollTop
    scrollTop = cursorRow
    // Deleting lines never feeds scrollback.
    scrollUp(count, feedsScrollback: false)
    scrollTop = top
    cursorCol = 0
  }

  private mutating func deleteCharacters(_ count: Int) {
    pendingWrap = false
    let index = cursorLine
    guard lines[index].cells.count > cursorCol else { return }
    let end = min(lines[index].cells.count, cursorCol + count)
    lines[index].cells.removeSubrange(cursorCol..<end)
  }

  private mutating func insertCharacters(_ count: Int) {
    pendingWrap = false
    let index = cursorLine
    guard lines[index].cells.count > cursorCol else { return }
    lines[index].cells.insert(
      contentsOf: repeatElement(TerminalCell.blank(style), count: count), at: cursorCol)
    if lines[index].cells.count > cols { lines[index].cells.removeSubrange(cols...) }
  }

  private mutating func eraseCharacters(_ count: Int) {
    pendingWrap = false
    let index = cursorLine
    let end = min(cols, cursorCol + count)
    for column in cursorCol..<end where column < lines[index].cells.count {
      lines[index].cells[column] = .blank(style)
    }
  }

  private mutating func reset() {
    let size = (cols, rows)
    self = TerminalScreen(cols: size.0, rows: size.1, scrollback: scrollback)
  }

  // MARK: Size

  /// Takes a new size. Lines keep their length: they are cut when drawn, not rewrapped.
  public mutating func resize(cols newCols: Int, rows newRows: Int) {
    let newCols = max(2, newCols)
    let newRows = max(1, newRows)
    guard newCols != cols || newRows != rows else { return }
    if newRows > rows {
      lines.append(contentsOf: repeatElement(TerminalLine(), count: newRows - rows))
    } else if newRows < rows {
      var excess = rows - newRows
      // Blank lines under the cursor go first, then the top of the screen scrolls away.
      while excess > 0, lines.count - 1 > cursorLine, lines.last?.cells.isEmpty == true {
        lines.removeLast()
        excess -= 1
      }
      cursorRow -= excess
    }
    rows = newRows
    cols = newCols
    normalize()
    scrollTop = 0
    scrollBottom = rows - 1
    pendingWrap = false
    cursorRow = min(max(0, cursorRow), rows - 1)
    cursorCol = min(cursorCol, cols - 1)
    if let main = alternate {
      // The saved main screen is resized when it comes back.
      alternate = (main.lines, min(main.row, rows - 1), min(main.col, cols - 1))
    }
  }

  private mutating func normalize() {
    if lines.count < rows {
      lines.append(contentsOf: repeatElement(TerminalLine(), count: rows - lines.count))
    }
  }

  // MARK: Drawing

  /// One line as styled runs. With `cursor`, the cell under the cursor is drawn inverted.
  public func runs(line index: Int, cursor: Bool = false) -> [TerminalRun] {
    guard lines.indices.contains(index) else { return [] }
    var cells = lines[index].cells
    let showsCursor = cursor && cursorVisible && index == cursorLine
    if showsCursor {
      if cells.count <= cursorCol {
        cells.append(
          contentsOf: repeatElement(TerminalCell(" "), count: cursorCol - cells.count + 1))
      }
      var cell = cells[cursorCol]
      if cell.continuation, cursorCol > 0 { cell = cells[cursorCol - 1] }
      cell.style.inverse.toggle()
      cells[cursorCol] = cell
      if cursorCol + 1 < cells.count, cells[cursorCol + 1].continuation {
        cells[cursorCol + 1].style.inverse.toggle()
      }
    }
    var runs: [TerminalRun] = []
    for cell in cells where !cell.continuation {
      if let last = runs.last, last.style == cell.style {
        runs[runs.count - 1].text += cell.text
      } else {
        runs.append(TerminalRun(text: cell.text, style: cell.style))
      }
    }
    return runs
  }

  /// The whole buffer as plain text, for copying and tests.
  public var text: String {
    var all = lines.map(\.text)
    while all.last?.isEmpty == true { all.removeLast() }
    return all.joined(separator: "\n")
  }
}

/// Keys the terminal sends as escape sequences.
public enum TerminalKey: Sendable {
  case up, down, right, left, home, end, pageUp, pageDown, forwardDelete
  case escape, tab, enter, backspace

  public func bytes(applicationCursor: Bool = false) -> [UInt8] {
    let cursor = applicationCursor ? "\u{1B}O" : "\u{1B}["
    switch self {
    case .up: return Array((cursor + "A").utf8)
    case .down: return Array((cursor + "B").utf8)
    case .right: return Array((cursor + "C").utf8)
    case .left: return Array((cursor + "D").utf8)
    case .home: return Array((cursor + "H").utf8)
    case .end: return Array((cursor + "F").utf8)
    case .pageUp: return Array("\u{1B}[5~".utf8)
    case .pageDown: return Array("\u{1B}[6~".utf8)
    case .forwardDelete: return Array("\u{1B}[3~".utf8)
    case .escape: return [0x1B]
    case .tab: return [0x09]
    case .enter: return [0x0D]
    case .backspace: return [0x7F]
    }
  }
}

/// Turns typed text into terminal input, with the Control and Option modifiers.
public enum TerminalInput {
  /// The control code for Control plus a character (`c` → 0x03), if it has one.
  public static func control(_ character: Character) -> UInt8? {
    guard let ascii = character.asciiValue else { return nil }
    switch ascii {
    case 0x61...0x7A: return ascii - 0x60  // a–z
    case 0x40...0x5F: return ascii - 0x40  // @ A–Z [ \ ] ^ _
    case 0x20, 0x32: return 0x00  // space, 2
    case 0x3F: return 0x7F  // ?
    case 0x33: return 0x1B  // 3
    case 0x34: return 0x1C  // 4
    case 0x35: return 0x1D  // 5
    case 0x36: return 0x1E  // 6
    case 0x37, 0x2F: return 0x1F  // 7 /
    case 0x38: return 0x7F  // 8
    default: return nil
    }
  }

  /// Bytes for typed text. A newline is sent as Return; Control applies to the first
  /// character and Option prefixes Escape, as terminals do for Meta.
  public static func bytes(_ text: String, control: Bool = false, option: Bool = false)
    -> [UInt8]
  {
    var output: [UInt8] = []
    for (offset, character) in text.enumerated() {
      var bytes: [UInt8]
      if character == "\n" || character == "\r\n" {
        bytes = [0x0D]
      } else if control && offset == 0, let code = Self.control(character) {
        bytes = [code]
      } else {
        bytes = Array(String(character).utf8)
      }
      if option && offset == 0 { bytes.insert(0x1B, at: 0) }
      output += bytes
    }
    return output
  }

  /// Pasted text, wrapped in bracketed-paste markers when the program asked for them.
  public static func paste(_ text: String, bracketed: Bool) -> [UInt8] {
    let body = Array(
      text.replacingOccurrences(of: "\r\n", with: "\r").replacingOccurrences(
        of: "\n", with: "\r"
      ).utf8)
    return bracketed ? Array("\u{1B}[200~".utf8) + body + Array("\u{1B}[201~".utf8) : body
  }
}
