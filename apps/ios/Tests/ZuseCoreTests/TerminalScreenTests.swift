import Foundation
import Testing

@testable import ZuseCore

private func screen(_ output: String, cols: Int = 20, rows: Int = 5) -> TerminalScreen {
  var screen = TerminalScreen(cols: cols, rows: rows)
  _ = screen.feed(output)
  return screen
}

@Test func terminalWritesLinesAndMovesTheCursor() {
  let shown = screen("$ ls\r\nREADME.md  src\r\n$ ")
  #expect(shown.text == "$ ls\nREADME.md  src\n$ ")
  #expect(shown.cursorRow == 2)
  #expect(shown.cursorCol == 2)
}

@Test func terminalAppliesColoursAndAttributes() {
  let shown = screen(
    "\u{1B}[1;32mok\u{1B}[0m \u{1B}[38;5;208mwarm\u{1B}[39m \u{1B}[48;2;1;2;3mx\u{1B}[m")
  let runs = shown.runs(line: shown.screenTop)
  #expect(runs.map(\.text) == ["ok", " ", "warm", " ", "x"])
  #expect(runs[0].style == TerminalStyle(foreground: .indexed(2), bold: true))
  #expect(runs[1].style == .plain)
  #expect(runs[2].style.foreground == .indexed(208))
  #expect(runs[4].style.background == .rgb(1, 2, 3))
  let bright = screen("\u{1B}[94mblue\u{1B}[7mrev")
  #expect(bright.runs(line: bright.screenTop)[0].style.foreground == .indexed(12))
  #expect(bright.runs(line: bright.screenTop)[1].style.inverse)
}

@Test func carriageReturnOverwritesAndEraseClearsTheRest() {
  // Progress lines redraw in place; erase-in-line drops what the new text didn't cover.
  let shown = screen("Downloading 10%\rDownloading 100%\r\u{1B}[KDone")
  #expect(shown.text == "Done")
  let partial = screen("abcdef\u{1B}[3D\u{1B}[K")
  #expect(partial.text == "abc")
  let backspace = screen("ab\u{8}c")
  #expect(backspace.text == "ac")
}

@Test func longLinesWrapOnlyWhenTheNextCharacterArrives() {
  // zsh pads its missing-newline marker to the full width, returns and clears; that mustn't add
  // a line.
  let marker = screen(
    "\u{1B}[7m%\u{1B}[27m" + String(repeating: " ", count: 9) + "\r \r\u{1B}[J$ ", cols: 10)
  #expect(marker.text == "$ ")
  let wrapped = screen("0123456789abc", cols: 10)
  #expect(wrapped.text == "0123456789\nabc")
  #expect(wrapped.cursorRow == 1 && wrapped.cursorCol == 3)
}

@Test func outputPastTheBottomScrollsIntoScrollback() {
  var shown = TerminalScreen(cols: 10, rows: 3, scrollback: 2)
  _ = shown.feed((1...8).map { "line\($0)" }.joined(separator: "\r\n"))
  #expect(shown.lines.count == 5)
  #expect(shown.text == "line4\nline5\nline6\nline7\nline8")
  #expect(shown.cursorRow == 2)
}

@Test func cursorPositioningAndClearingTheScreen() {
  var shown = TerminalScreen(cols: 10, rows: 3)
  _ = shown.feed("one\r\ntwo\r\nthree\u{1B}[2;2HX\u{1B}[1;1H\u{1B}[0K")
  #expect(shown.text == "\ntXo\nthree")
  _ = shown.feed("\u{1B}[H\u{1B}[2J\u{1B}[3J$ ")
  #expect(shown.text == "$ ")
  #expect(shown.lines.count == 3)
}

@Test func terminalAnswersCursorAndDeviceQueries() {
  var shown = TerminalScreen(cols: 10, rows: 3)
  let reply = shown.feed("ab\u{1B}[6n")
  #expect(String(decoding: reply, as: UTF8.self) == "\u{1B}[1;3R")
  #expect(String(decoding: shown.feed("\u{1B}[c"), as: UTF8.self) == "\u{1B}[?1;2c")
  #expect(shown.feed("plain").isEmpty)
}

@Test func titlesAndUnknownSequencesDrawNothing() {
  let shown = screen(
    "\u{1B}]2;user@host:~\u{7}\u{1B}]7;file://host/tmp\u{1B}\\\u{1B}[?1h\u{1B}=\u{1B}[?2004h\u{1B}[ q\u{1B}(Bok"
  )
  #expect(shown.text == "ok")
  #expect(shown.title == "user@host:~")
  #expect(shown.applicationCursorKeys)
  #expect(shown.bracketedPaste)
}

@Test func utf8SplitAcrossReadsAndWideCharacters() {
  var shown = TerminalScreen(cols: 10, rows: 2)
  let bytes = Array("é漢x".utf8)
  _ = shown.feed(Array(bytes[0..<1]))
  _ = shown.feed(Array(bytes[1..<3]))
  _ = shown.feed(Array(bytes[3...]))
  #expect(shown.text == "é漢x")
  // The wide character takes two columns.
  #expect(shown.cursorCol == 4)
  let combining = screen("e\u{301}!")
  #expect(combining.text == "e\u{301}!")
  #expect(combining.cursorCol == 2)
}

@Test func alternateScreenRestoresTheShell() {
  var shown = TerminalScreen(cols: 10, rows: 3)
  _ = shown.feed("$ less\r\n")
  _ = shown.feed("\u{1B}[?1049h\u{1B}[Hpage one\u{1B}[2;1Hpage two")
  #expect(shown.usesAlternateScreen)
  #expect(shown.text == "page one\npage two")
  _ = shown.feed("\u{1B}[?1049l")
  #expect(!shown.usesAlternateScreen)
  #expect(shown.text == "$ less")
  #expect(shown.cursorRow == 1 && shown.cursorCol == 0)
}

@Test func scrollRegionsAndLineEditing() {
  var shown = TerminalScreen(cols: 10, rows: 4)
  _ = shown.feed("a\r\nb\r\nc\r\nd")
  _ = shown.feed("\u{1B}[2;3r\u{1B}[3;1H\n")
  #expect(shown.text == "a\nc\n\nd")
  _ = shown.feed("\u{1B}[r\u{1B}[1;1H\u{1B}[1M")
  #expect(shown.text == "c\n\nd")
  _ = shown.feed("\u{1B}[1;1Hxyz\u{1B}[1;2H\u{1B}[1P")
  #expect(shown.lines[shown.screenTop].text == "xz")
}

@Test func resizingKeepsTheCursorOnScreen() {
  var shown = TerminalScreen(cols: 10, rows: 5)
  _ = shown.feed("1\r\n2\r\n3")
  shown.resize(cols: 6, rows: 2)
  #expect(shown.rows == 2 && shown.cols == 6)
  #expect(shown.cursorRow == 1)
  #expect(shown.lines[shown.cursorLine].text == "3")
  shown.resize(cols: 6, rows: 4)
  #expect(shown.lines[shown.cursorLine].text == "3")
  #expect(shown.lines.count >= 4)
}

@Test func cursorIsDrawnInverted() {
  let shown = screen("ab")
  let runs = shown.runs(line: shown.cursorLine, cursor: true)
  #expect(runs.map(\.text) == ["ab", " "])
  #expect(runs.last?.style.inverse == true)
  let hidden = screen("ab\u{1B}[?25l")
  #expect(hidden.runs(line: hidden.cursorLine, cursor: true).map(\.text) == ["ab"])
}

@Test func keysAndModifiersBecomeTerminalInput() {
  #expect(TerminalKey.up.bytes() == Array("\u{1B}[A".utf8))
  #expect(TerminalKey.up.bytes(applicationCursor: true) == Array("\u{1B}OA".utf8))
  #expect(TerminalKey.backspace.bytes() == [0x7F])
  #expect(TerminalInput.control("c") == 0x03)
  #expect(TerminalInput.control("C") == 0x03)
  #expect(TerminalInput.control("[") == 0x1B)
  #expect(TerminalInput.bytes("c", control: true) == [0x03])
  #expect(TerminalInput.bytes("b", option: true) == [0x1B, 0x62])
  #expect(TerminalInput.bytes("ls\n") == Array("ls\r".utf8))
  #expect(
    TerminalInput.paste("a\nb", bracketed: true) == Array("\u{1B}[200~a\rb\u{1B}[201~".utf8))
}
