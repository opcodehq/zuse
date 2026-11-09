import XCTest

final class NativeMacTests: XCTestCase {
  @MainActor func testNativeSettingsShortcutAndCompactComposer() {
    let app = XCUIApplication()
    app.launchArguments = ["--native-preview", "--conversation-preview"]
    app.launch()
    let composer = app.descendants(matching: .any).matching(identifier: "chat-composer").firstMatch
    XCTAssertTrue(composer.waitForExistence(timeout: 20))
    XCTAssertFalse(app.buttons["Send message"].isEnabled)
    app.typeKey(",", modifierFlags: .command)
    XCTAssertTrue(app.buttons["Sign in to Zuse"].waitForExistence(timeout: 5))
    XCTAssertTrue(app.staticTexts["Appearance"].exists)
    let screenshot = XCTAttachment(screenshot: app.screenshot())
    screenshot.name = "Native Mac settings"
    screenshot.lifetime = .keepAlways
    add(screenshot)
  }

  @MainActor func testLargeTranscriptStartsAtLatestAndKeepsComposerVisible() {
    let app = XCUIApplication()
    app.launchArguments = ["--native-preview", "--large-transcript"]
    app.launch()
    let transcript = app.tables["chat-transcript"]
    XCTAssertTrue(transcript.waitForExistence(timeout: 15))
    let last = transcript.rows.element(boundBy: 9_999)
    XCTAssertTrue(last.waitForExistence(timeout: 10))
    XCTAssertTrue(last.isHittable, "The transcript must open at its latest message")
    XCTAssertTrue(last.staticTexts["Review the changes for task 9999."].exists)
    transcript.scroll(byDeltaX: 0, deltaY: 600)
    XCTAssertTrue(
      app.descendants(matching: .any).matching(identifier: "chat-composer").firstMatch.exists)
    let screenshot = XCTAttachment(screenshot: app.screenshot())
    screenshot.name = "Native Mac large transcript"
    screenshot.lifetime = .keepAlways
    add(screenshot)
  }
}
