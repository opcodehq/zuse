import XCTest

final class NativeChatTests: XCTestCase {
  @MainActor func testSampleModelPickerChangesOnlyPreviewSelection() throws {
    let app = XCUIApplication()
    app.launchArguments = ["--native-preview", "--conversation-preview", "--model-picker-preview"]
    app.launch()
    let picker = app.buttons["Current model: sample-balanced"]
    XCTAssertTrue(picker.waitForExistence(timeout: 15))
    XCTAssertTrue(picker.isEnabled)
    picker.tap()
    let effort = app.sliders["Reasoning effort"]
    XCTAssertTrue(effort.waitForExistence(timeout: 5))
    effort.adjust(toNormalizedSliderPosition: 1)
    XCTAssertEqual(effort.value as? String, "Max")
    app.buttons["model-model-menu"].coordinate(withNormalizedOffset: CGVector(dx: 0.85, dy: 0.5))
      .tap()
    XCTAssertTrue(app.buttons["selection-choice-sample-fast"].waitForExistence(timeout: 5))
    XCTAssertTrue(app.buttons["selection-choice-sample-reasoning"].exists)
    XCTAssertTrue(app.buttons["selection-choice-sample-code"].exists)
    let capture = XCTAttachment(screenshot: app.screenshot())
    capture.name = "Sample model picker"
    capture.lifetime = .keepAlways
    add(capture)
    app.buttons["selection-choice-sample-reasoning"].tap()
    app.buttons["model-provider-menu"].coordinate(withNormalizedOffset: CGVector(dx: 0.85, dy: 0.5))
      .tap()
    app.buttons["selection-choice-sample-anthropic"].tap()
    app.buttons["model-model-menu"].coordinate(withNormalizedOffset: CGVector(dx: 0.85, dy: 0.5))
      .tap()
    let sonnet = app.buttons["selection-choice-sample-sonnet"]
    XCTAssertTrue(sonnet.waitForExistence(timeout: 5))
    sonnet.tap()
    app.buttons["Close"].tap()
    XCTAssertTrue(app.buttons["Current model: sample-sonnet"].waitForExistence(timeout: 5))
    XCTAssertTrue(app.buttons["tool-group-turn-2:t0:activity"].exists)
    XCTAssertFalse(app.buttons["Send message"].isEnabled)
  }

  @MainActor func testConversationPreviewExpandsToolCalls() throws {
    let app = XCUIApplication()
    app.launchArguments = ["--native-preview", "--conversation-preview"]
    app.launch()
    let transcript = app.collectionViews["chat-transcript"]
    XCTAssertTrue(transcript.waitForExistence(timeout: 15))
    // The latest turn folds its work behind one line: timing, then what it did.
    let heading = app.buttons["tool-group-turn-2:t0:activity"]
    XCTAssertTrue(heading.waitForExistence(timeout: 10))
    XCTAssertTrue(heading.label.hasPrefix("Worked for 21s · Ran 2 commands"))
    XCTAssertFalse(app.buttons["activity-reply-2:x2"].exists)
    heading.tap()
    let run = app.buttons["activity-reply-2:x2"]
    XCTAssertTrue(run.waitForExistence(timeout: 5))
    XCTAssertLessThanOrEqual(run.frame.height, 36)
    run.tap()
    XCTAssertTrue(
      app.descendants(matching: .any).matching(
        NSPredicate(
          format: "label CONTAINS %@ OR value CONTAINS %@", "Executed 214 tests",
          "Executed 214 tests")
      ).firstMatch.waitForExistence(timeout: 5))
    let screenshot = XCTAttachment(screenshot: app.screenshot())
    screenshot.name = "Expanded tool call and composer"
    screenshot.lifetime = .keepAlways
    add(screenshot)
    XCTAssertTrue(app.textFields["chat-composer"].exists)
    XCTAssertFalse(app.buttons["Send message"].isEnabled)
  }

  @MainActor func testComposerCollapsesAndNewChatHasDestinationInside() throws {
    let app = XCUIApplication()
    app.launchArguments = ["--native-preview", "--conversation-preview"]
    app.launch()
    let collapsed = app.otherElements["composer-collapsed"]
    XCTAssertTrue(collapsed.waitForExistence(timeout: 15))
    XCTAssertLessThanOrEqual(collapsed.frame.height, 58)
    let collapsedHeight = collapsed.frame.height
    let input = app.textFields["chat-composer"]
    XCTAssertLessThan(app.staticTexts["Offline preview"].frame.maxY, input.frame.minY)
    let screenshot = XCTAttachment(screenshot: app.screenshot())
    screenshot.name = "Compact conversation"
    screenshot.lifetime = .keepAlways
    add(screenshot)
    input.tap()
    input.typeText("Review the changes")
    let expanded = app.otherElements["composer-expanded"]
    XCTAssertTrue(expanded.waitForExistence(timeout: 5))
    XCTAssertGreaterThan(expanded.frame.height, collapsedHeight)
    let draft = XCTAttachment(screenshot: app.screenshot())
    draft.name = "Expanded composer"
    draft.lifetime = .keepAlways
    add(draft)

    app.terminate()
    app.launchArguments = ["--native-preview", "--home-preview", "--tab=chats"]
    app.launch()
    app.buttons["New chat options"].tap()
    app.buttons["New chat"].tap()
    XCTAssertTrue(app.navigationBars["New Chat"].waitForExistence(timeout: 5))
    let newPrompt = app.textFields["new-chat-prompt"]
    XCTAssertTrue(newPrompt.waitForExistence(timeout: 5))
    newPrompt.tap()
    newPrompt.typeText("Keep this draft")
    app.buttons["new-chat-destination"].tap()
    app.buttons["Cloud"].tap()
    XCTAssertTrue(app.buttons["Sign in to Zuse"].waitForExistence(timeout: 5))
    XCTAssertEqual(app.textFields["new-chat-prompt"].value as? String, "Keep this draft")
    let setup = XCTAttachment(screenshot: app.screenshot())
    setup.name = "New chat destination"
    setup.lifetime = .keepAlways
    add(setup)
  }

  @MainActor func testHomePreviewOpensConversation() throws {
    let app = XCUIApplication()
    app.launchArguments = ["--native-preview", "--home-preview", "--tab=chats"]
    app.launch()
    let row = app.staticTexts["Polish the mobile navigation"]
    XCTAssertTrue(row.waitForExistence(timeout: 15))
    row.tap()
    XCTAssertTrue(app.collectionViews["chat-transcript"].waitForExistence(timeout: 10))
    XCTAssertFalse(app.buttons["Send message"].isEnabled)
  }

  @MainActor func testTabsFilterChatsAndReachComputers() throws {
    let app = XCUIApplication()
    app.launchArguments = ["--native-preview", "--home-preview", "--tab=chats"]
    app.launch()
    XCTAssertTrue(app.staticTexts["Polish the mobile navigation"].waitForExistence(timeout: 15))
    XCTAssertFalse(app.buttons["Sign in to Zuse"].exists)
    XCTAssertFalse(app.buttons["Connect a computer"].exists)
    let archivedRow = app.buttons["Try the old sidebar, archived"]
    XCTAssertFalse(archivedRow.exists)
    app.buttons["archive-filter"].tap()
    app.buttons["Archived"].firstMatch.tap()
    XCTAssertTrue(archivedRow.waitForExistence(timeout: 5))
    XCTAssertFalse(app.staticTexts["Polish the mobile navigation"].exists)
    app.buttons["Show all chats"].tap()
    XCTAssertTrue(app.staticTexts["Polish the mobile navigation"].waitForExistence(timeout: 5))
    // iPad lists Settings and Computers in its sidebar; iPhone keeps them under More.
    let sidebar = UIDevice.current.userInterfaceIdiom == .pad
    if sidebar { openSidebar(app) }
    app.buttons[sidebar ? "sidebar-settings" : "More"].firstMatch.tap()
    XCTAssertTrue(app.buttons["more-account"].waitForExistence(timeout: 5))
    XCTAssertTrue(app.staticTexts["Sign in"].exists)
    XCTAssertTrue(app.staticTexts["Archived chats"].exists)
    if sidebar { openSidebar(app) }
    app.buttons[sidebar ? "sidebar-computers" : "settings-computers"].firstMatch.tap()
    XCTAssertTrue(app.staticTexts["Connect your computer"].waitForExistence(timeout: 5))
    let computers = XCTAttachment(screenshot: app.screenshot())
    computers.name = "Computer connections"
    computers.lifetime = .keepAlways
    add(computers)
    app.buttons["Add computer"].tap()
    XCTAssertTrue(app.textFields["Invitation link"].waitForExistence(timeout: 5))
    XCTAssertTrue(app.staticTexts["Connect with SSH"].exists)
    XCTAssertFalse(app.secureTextFields["Connection token"].exists)
    XCTAssertFalse(app.staticTexts["Nearby computers"].exists)
  }

  @MainActor func testSearchFindsChatsAndZappers() throws {
    let app = XCUIApplication()
    app.launchArguments = [
      "--native-preview", "--home-preview", "--zapper-preview", "--search=authentication",
    ]
    app.launch()
    XCTAssertTrue(app.staticTexts["Review authentication changes"].waitForExistence(timeout: 15))
    XCTAssertFalse(app.staticTexts["Polish the mobile navigation"].exists)
    let search = app.searchFields.firstMatch
    XCTAssertTrue(search.waitForExistence(timeout: 5))
    search.tap()
    search.buttons["Clear text"].tap()
    search.typeText("Release")
    let zapper = app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'Release lead'"))
    XCTAssertTrue(zapper.firstMatch.waitForExistence(timeout: 5))
  }

  @MainActor func testTranscriptScrollsWithKeyboardAndDraftRestsCompactly() throws {
    let app = XCUIApplication()
    app.launchArguments = ["--native-preview", "--large-transcript"]
    app.launch()
    let latest = app.staticTexts["Review the changes for task 9999."]
    XCTAssertTrue(latest.waitForExistence(timeout: 20))
    let input = app.textFields["chat-composer"]
    input.tap()
    input.typeText("Keep this draft")
    XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5))
    let transcript = app.collectionViews["chat-transcript"]
    let before = latest.frame.minY
    transcript.swipeDown()
    XCTAssertTrue(!latest.exists || abs(latest.frame.minY - before) > 20)
    if app.keyboards.firstMatch.exists { transcript.swipeDown() }
    let resting = app.otherElements["composer-collapsed"]
    XCTAssertTrue(resting.waitForExistence(timeout: 5))
    XCTAssertLessThanOrEqual(resting.frame.height, 48)
    XCTAssertEqual(input.value as? String, "Keep this draft")
  }

  @MainActor func testLargeTranscriptOpensAtLatestMessageAndScrolls() throws {
    let app = XCUIApplication()
    app.launchArguments = ["--native-preview", "--large-transcript"]
    app.launch()
    XCTAssertTrue(
      app.staticTexts["Review the changes for task 9999."].waitForExistence(timeout: 20))
    let transcript = app.collectionViews["chat-transcript"]
    XCTAssertTrue(transcript.exists)
    transcript.swipeDown()
    transcript.swipeDown()
    XCTAssertTrue(app.textFields["chat-composer"].exists)
    XCTAssertFalse(app.buttons["Send message"].isEnabled)
    let attachment = XCTAttachment(screenshot: app.screenshot())
    attachment.name = "Native transcript"
    attachment.lifetime = .keepAlways
    add(attachment)
  }
  @MainActor func testGeneratedVisualRendersNatively() throws {
    let app = XCUIApplication()
    app.launchArguments = ["--native-preview", "--visual-preview"]
    app.launch()
    XCTAssertTrue(app.staticTexts["Project overview"].waitForExistence(timeout: 20))
    XCTAssertTrue(app.staticTexts["Completed tasks"].exists)
    XCTAssertTrue(app.staticTexts["Build checks"].exists)
    let attachment = XCTAttachment(screenshot: app.screenshot())
    attachment.name = "Native visual"
    attachment.lifetime = .keepAlways
    add(attachment)
  }
  @MainActor func testConnectedChatSendsAndShowsDiff() throws {
    guard let url = ProcessInfo.processInfo.environment["ZUSE_IOS_FIXTURE_URL"] else {
      throw XCTSkip("Run scripts/test-simulator.sh to start the local RPC fixture.")
    }
    let app = XCUIApplication()
    app.launchArguments = ["--rpc-fixture", "--tab=chats"]
    app.launchEnvironment["ZUSE_IOS_FIXTURE_URL"] = url
    app.launch()
    let chat = app.buttons["Fixture conversation"]
    if chat.waitForExistence(timeout: 10) {
      chat.tap()
    } else {
      let link = app.staticTexts["Fixture conversation"].firstMatch
      XCTAssertTrue(link.waitForExistence(timeout: 10))
      link.tap()
    }
    let composer = app.textFields["chat-composer"]
    XCTAssertTrue(composer.waitForExistence(timeout: 15))
    composer.tap()
    composer.typeText("Hello from native UI")
    app.buttons["Send message"].tap()
    XCTAssertTrue(app.staticTexts["Received: Hello from native UI"].waitForExistence(timeout: 10))
    // Changes lives in the chat's ··· sheet and is enabled once the chat is connected.
    app.buttons["chat-actions"].tap()
    let changes = app.buttons["Changes"]
    XCTAssertTrue(changes.waitForExistence(timeout: 15))
    wait(
      for: [expectation(for: NSPredicate(format: "enabled == true"), evaluatedWith: changes)],
      timeout: 15)
    changes.tap()
    XCTAssertTrue(app.staticTexts["example.swift"].waitForExistence(timeout: 10))
    XCTAssertTrue(app.staticTexts["+let value = 2"].exists)
    let diffCapture = XCTAttachment(screenshot: app.screenshot())
    diffCapture.name = "Compact diff"
    diffCapture.lifetime = .keepAlways
    add(diffCapture)
  }
  @MainActor func testDiagramRendersOffline() throws {
    let app = XCUIApplication()
    app.launchArguments = ["--native-preview", "--diagram-preview"]
    app.launch()
    XCTAssertTrue(app.webViews.firstMatch.waitForExistence(timeout: 15))
    XCTAssertTrue(app.webViews.staticTexts["Complete"].waitForExistence(timeout: 15))
    XCTAssertTrue(app.buttons["Expand diagram"].exists)
    let attachment = XCTAttachment(screenshot: app.screenshot())
    attachment.name = "Offline diagram"
    attachment.lifetime = .keepAlways
    add(attachment)
  }
  @MainActor func testConnectedNewChatCreatesAndSendsWithNativeComposer() throws {
    guard let url = ProcessInfo.processInfo.environment["ZUSE_IOS_FIXTURE_URL"] else {
      throw XCTSkip("Run scripts/test-simulator.sh to start the local RPC fixture.")
    }
    let app = XCUIApplication()
    app.launchArguments = ["--rpc-fixture", "--tab=chats"]
    app.launchEnvironment["ZUSE_IOS_FIXTURE_URL"] = url
    app.launch()
    XCTAssertTrue(app.staticTexts["Fixture conversation"].firstMatch.waitForExistence(timeout: 15))
    XCTAssertTrue(app.staticTexts["#42"].waitForExistence(timeout: 10))
    app.buttons["chats-more"].tap()
    app.buttons["New chat"].tap()
    XCTAssertTrue(app.buttons["new-chat-destination"].waitForExistence(timeout: 5))
    let create = app.buttons["new-chat-create"]
    XCTAssertTrue(create.waitForExistence(timeout: 10))
    let ready = expectation(for: NSPredicate(format: "enabled == true"), evaluatedWith: create)
    wait(for: [ready], timeout: 10)
    let firstPrompt = app.textFields["new-chat-prompt"]
    firstPrompt.tap()
    firstPrompt.typeText("Start with this prompt")
    create.tap()
    XCTAssertTrue(app.staticTexts["Start with this prompt"].waitForExistence(timeout: 10))
    let composer = app.textFields["chat-composer"]
    XCTAssertTrue(composer.waitForExistence(timeout: 15))
    composer.tap()
    composer.typeText("A fresh native chat")
    app.buttons["Send message"].tap()
    XCTAssertTrue(app.staticTexts["Received: A fresh native chat"].waitForExistence(timeout: 10))
  }

  /// In portrait the iPad sidebar is tucked away; its toolbar button brings it back.
  @MainActor private func openSidebar(_ app: XCUIApplication) {
    let settings = app.buttons["sidebar-settings"].firstMatch
    if settings.waitForExistence(timeout: 2) && settings.isHittable { return }
    app.buttons["Show Sidebar"].firstMatch.tap()
    XCTAssertTrue(settings.waitForExistence(timeout: 5))
  }
}
