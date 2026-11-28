import XCTest

/// The pages beside a chat: swiping between them, the overview, and closing a pane.
final class ChatPanesTests: XCTestCase {
  override func setUp() {
    continueAfterFailure = false
  }

  /// Waits for the page dots under the title to say `label`.
  private func expectDots(
    _ app: XCUIApplication, _ label: String, file: StaticString = #filePath, line: UInt = #line
  ) {
    let dots = app.descendants(matching: .any)["pane-dots"].firstMatch
    // An ordinary chat's title reads its dots along with its name.
    let predicate = NSPredicate(format: "label ENDSWITH %@", label)
    let result = XCTWaiter.wait(
      for: [XCTNSPredicateExpectation(predicate: predicate, object: dots)], timeout: 8)
    XCTAssertEqual(result, .completed, "Dots say “\(dots.label)”", file: file, line: line)
  }

  private func waitToDisappear(_ element: XCUIElement) -> Bool {
    XCTWaiter.wait(
      for: [
        XCTNSPredicateExpectation(
          predicate: NSPredicate(format: "exists == false"), object: element)
      ],
      timeout: 8) == .completed
  }

  /// Keeps a screenshot with the test, and in `PANES_SHOTS` when that is set.
  private func capture(_ app: XCUIApplication, _ name: String) {
    let shot = app.screenshot()
    let attachment = XCTAttachment(screenshot: shot)
    attachment.name = name
    attachment.lifetime = .keepAlways
    add(attachment)
    if let folder = ProcessInfo.processInfo.environment["PANES_SHOTS"], !folder.isEmpty {
      try? shot.pngRepresentation.write(to: URL(fileURLWithPath: folder + "/" + name + ".png"))
    }
  }

  @MainActor func testSwipingBetweenPanesAndClosingOne() throws {
    let app = XCUIApplication()
    // The sample chat opens with a terminal, files, side threads, a pull request and a file.
    app.launchArguments = [
      "--native-preview", "--conversation-preview", "--panes-preview", "--pane=files",
    ]
    app.launch()
    XCTAssertTrue(
      app.descendants(matching: .any)["pane-dots"].firstMatch.waitForExistence(timeout: 15))
    expectDots(app, "Page 3 of 7")
    let search = app.textFields["Search files"]
    XCTAssertTrue(search.waitForExistence(timeout: 5))

    // Sideways swipes page through the panes; the list's own scrolling stays vertical.
    let window = app.windows.firstMatch
    let middle = window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.62))
    // A flick, as a finger would: a slow drag short of half a page springs back.
    middle.press(
      forDuration: 0.05,
      thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: 0.05, dy: 0.62)),
      withVelocity: .fast, thenHoldForDuration: 0)
    expectDots(app, "Page 4 of 7")
    middle.press(
      forDuration: 0.05,
      thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.62)),
      withVelocity: .fast, thenHoldForDuration: 0)
    expectDots(app, "Page 3 of 7")

    // Pinching in zooms out to every page as a card. The iPad simulator doesn't deliver
    // XCTest's synthesized pinch, so there tapping the page dots opens the overview.
    let toggle = app.buttons["pane-dots"]
    if UIDevice.current.userInterfaceIdiom == .pad {
      toggle.tap()
    } else {
      window.pinch(withScale: 0.45, velocity: -1.5)
    }
    XCTAssertTrue(
      app.buttons["pane-card-main"].waitForExistence(timeout: 5), "The overview didn't open")
    capture(app, "panes-overview")

    // Swiping the front card up closes it and brings the one before it forward.
    let files = app.buttons["pane-card-preview-files"]
    let card = files.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.6))
    card.press(
      forDuration: 0.05, thenDragTo: card.withOffset(CGVector(dx: 0, dy: -420)),
      withVelocity: .fast, thenHoldForDuration: 0)
    XCTAssertTrue(waitToDisappear(files))
    expectDots(app, "Page 2 of 6")
    // The chat and the picker can't be closed.
    XCTAssertTrue(app.buttons["pane-card-main"].exists)
    XCTAssertTrue(
      app.buttons["pane-card-selection"].exists || app.buttons["pane-tile-files"].exists)

    // Tapping a card brings its page back full screen.
    app.buttons["pane-card-preview-terminal"].tap()
    let closeTerminal = app.buttons["Close Terminal"]
    XCTAssertTrue(closeTerminal.waitForExistence(timeout: 5))
    capture(app, "panes-terminal-page")

    // A pane's own close button closes it too.
    closeTerminal.tap()
    expectDots(app, "Page 1 of 5")
  }

  @MainActor func testPickerOpensAPaneAfterTheOthers() throws {
    let app = XCUIApplication()
    app.launchArguments = [
      "--native-preview", "--zapper-preview", "--zapper-detail-preview", "--zapper-chat-preview",
      "--panes-preview", "--pane=selection",
    ]
    app.launch()
    // Threads, pull requests, issues, terminal, files, docs and a document are open.
    XCTAssertTrue(
      app.descendants(matching: .any)["pane-dots"].firstMatch.waitForExistence(timeout: 15))
    expectDots(app, "Page 9 of 9")
    let side = app.buttons["pane-tile-sideThreads"]
    XCTAssertTrue(side.waitForExistence(timeout: 5))
    capture(app, "panes-picker")
    side.tap()
    // It opens after the existing panes, before the picker, and comes to the front.
    expectDots(app, "Page 9 of 10")
    XCTAssertTrue(app.buttons["Close Side threads"].waitForExistence(timeout: 5))
    // Asking for a list that is already open brings it forward instead of opening it twice.
    window(app).swipeLeft()
    expectDots(app, "Page 10 of 10")
    app.buttons["pane-tile-threads"].tap()
    expectDots(app, "Page 2 of 10")
  }

  private func window(_ app: XCUIApplication) -> XCUIElement { app.windows.firstMatch }

  /// Holds a card halfway up for `PANES_HOLD` seconds, for a screenshot of the gesture, then
  /// lets it go; a card let go above the threshold closes.
  @MainActor func testLiftingACardHalfway() throws {
    let hold = Double(ProcessInfo.processInfo.environment["PANES_HOLD"] ?? "") ?? 0
    try XCTSkipIf(hold == 0, "Only for screenshots")
    let app = XCUIApplication()
    app.launchArguments = [
      "--native-preview", "--zapper-preview", "--zapper-detail-preview", "--zapper-chat-preview",
      "--panes-preview", "--pane=terminal", "--pane-overview",
    ]
    app.launch()
    let card = app.buttons["pane-card-preview-terminal"]
    XCTAssertTrue(card.waitForExistence(timeout: 15))
    let start = card.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.7))
    start.press(
      forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: 0, dy: -260)),
      withVelocity: .slow, thenHoldForDuration: hold)
    XCTAssertTrue(waitToDisappear(card))
  }

  /// Against a computer: `TEST_RUNNER_ZUSE_PANES_PAIR_LINK` and `TEST_RUNNER_ZUSE_PANES_CHAT`
  /// name a pairing invitation and a chat whose folder has `src/main.rs`.
  @MainActor func testLiveFilesAndTerminal() throws {
    let environment = ProcessInfo.processInfo.environment
    let link = environment["ZUSE_PANES_PAIR_LINK"] ?? ""
    let chat = environment["ZUSE_PANES_CHAT"] ?? ""
    try XCTSkipIf(link.isEmpty || chat.isEmpty, "Needs a paired computer")
    let app = XCUIApplication()
    app.launchEnvironment["ZUSE_PAIR_LINK"] = link
    app.launchArguments = ["--open-chat=\(chat)", "--panes-preview", "--pane=files"]
    app.launch()
    let source = app.buttons["file-row-src"]
    XCTAssertTrue(source.waitForExistence(timeout: 30))
    source.tap()
    let main = app.buttons["file-row-src/main.rs"]
    XCTAssertTrue(main.waitForExistence(timeout: 10))
    capture(app, "live-files-open-folder")
    main.tap()
    XCTAssertTrue(app.buttons["Close main.rs"].waitForExistence(timeout: 10))
    XCTAssertTrue(
      app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "println")).firstMatch
        .waitForExistence(timeout: 10))
    capture(app, "live-file")

    app.buttons["pane-dots"].tap()
    let card = app.buttons["pane-card-preview-terminal"]
    XCTAssertTrue(card.waitForExistence(timeout: 5))
    capture(app, "live-overview")
    // The cards scroll sideways; bring the terminal's into view.
    for _ in 0..<4 where card.frame.midX < 0 { window(app).swipeRight() }
    card.tap()
    let output = app.descendants(matching: .any)["terminal-output"]
    XCTAssertTrue(output.waitForExistence(timeout: 10))
    output.tap()
    XCTAssertTrue(app.buttons["Escape"].waitForExistence(timeout: 5))
    app.typeText("echo panes-$((6*7))\n")
    let answered = NSPredicate(format: "value CONTAINS %@", "panes-42")
    XCTAssertEqual(
      XCTWaiter.wait(
        for: [XCTNSPredicateExpectation(predicate: answered, object: output)], timeout: 10),
      .completed)
    capture(app, "live-terminal-typed")
  }
}
