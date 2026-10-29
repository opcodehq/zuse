#if os(macOS) || os(Linux)
  import Foundation
  import Testing
  #if os(macOS)
    import Darwin
  #else
    import Glibc
  #endif

  @testable import ZuseCore

  @Suite(.timeLimit(.minutes(1)))
  struct ServeCommandTests {
    @Test func serviceDoesNotInheritUnrelatedAppDescriptors() async throws {
      let file = open("/dev/null", O_RDONLY)
      #expect(file >= 0)
      defer { _ = close(file) }
      let descriptor = fcntl(file, F_DUPFD, 200)
      #expect(descriptor >= 200)
      defer { _ = close(descriptor) }
      let output = try await ServeCommand.run(
        executable: "/bin/sh",
        arguments: [
          "-c", "if [ -e \"/dev/fd/$1\" ]; then printf leaked; else printf closed; fi", "fixture",
          String(descriptor),
        ])
      #expect(output == "closed")
    }
    @Test func argumentsRemainLiteralWithoutShellExpansion() async throws {
      let value = "$(printf unexpected); `printf unexpected` with spaces"
      let output = try await ServeCommand.run(
        executable: "/usr/bin/printf", arguments: ["%s", value])
      #expect(output == value)
    }

    @Test func largeOutputIsDrainedWithBoundedCapture() async throws {
      let output = try await ServeCommand.run(
        executable: "/usr/bin/head", arguments: ["-c", "1000000", "/dev/zero"], captureLimit: 1024)
      #expect(output.utf8.count == 1024)
    }

    @Test func failuresDoNotExposeSubprocessCredentials() async throws {
      do {
        _ = try await ServeCommand.run(
          executable: "/bin/sh", arguments: ["-c", "printf private-pairing-code >&2; exit 17"])
        Issue.record("Expected the command to fail")
      } catch let error as ClientError {
        #expect(!error.message.contains("private-pairing-code"))
      }
    }

    @Test func successfulParentDoesNotWaitForAnInheritedOutputPipe() async throws {
      let clock = ContinuousClock()
      let started = clock.now
      let output = try await ServeCommand.run(
        executable: "/bin/sh", arguments: ["-c", "printf ready; sleep 3 & exit 0"],
        timeout: .seconds(1))
      #expect(output == "ready")
      #expect(started.duration(to: clock.now) < .seconds(2))
    }

    @Test func ignoredTerminationCannotKeepSettingsWaitingForever() async throws {
      let clock = ContinuousClock()
      let started = clock.now
      await #expect(throws: ClientError.self) {
        try await ServeCommand.run(
          executable: "/bin/sh", arguments: ["-c", "trap '' TERM; while :; do sleep 1; done"],
          timeout: .milliseconds(100))
      }
      #expect(started.duration(to: clock.now) < .seconds(3))
    }

    @Test func cancellationStopsAnAlreadyRunningCommand() async throws {
      let directory = FileManager.default.temporaryDirectory.appendingPathComponent(
        UUID().uuidString)
      try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
      defer { try? FileManager.default.removeItem(at: directory) }
      let marker = directory.appendingPathComponent("started")
      let task = Task {
        try await ServeCommand.run(
          executable: "/bin/sh",
          arguments: ["-c", "printf ready > \"$1\"; exec sleep 30", "fixture", marker.path])
      }
      let clock = ContinuousClock()
      let deadline = clock.now.advanced(by: .seconds(3))
      while !FileManager.default.fileExists(atPath: marker.path), clock.now < deadline {
        try await Task.sleep(for: .milliseconds(10))
      }
      #expect(FileManager.default.fileExists(atPath: marker.path))
      let cancelledAt = clock.now
      task.cancel()
      await #expect(throws: CancellationError.self) { _ = try await task.value }
      #expect(cancelledAt.duration(to: clock.now) < .seconds(3))
    }
  }
#endif
