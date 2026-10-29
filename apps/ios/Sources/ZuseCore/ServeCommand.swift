#if os(macOS) || os(Linux)
  import Foundation
  #if os(macOS)
    import Darwin
  #else
    import Glibc
  #endif

  /// Literal process arguments, bounded output and a deadline independent of pipe EOF.
  /// Only the launched CLI process is terminated; its daemon has its own lifecycle.
  public enum ServeCommand {
    private final class Cancellation: @unchecked Sendable {
      private let lock = NSLock()
      private var cancelled = false
      func cancel() { lock.withLock { cancelled = true } }
      var isCancelled: Bool { lock.withLock { cancelled } }
    }
    public static func run(
      executable: String, arguments: [String], timeout: Duration = .seconds(60),
      captureLimit: Int = 256 * 1024
    ) async throws -> String {
      let cancellation = Cancellation()
      return try await withTaskCancellationHandler {
        try Task.checkCancellation()
        return try await Task.detached(priority: .userInitiated) {
          try invoke(
            executable: executable, arguments: arguments, timeout: timeout,
            captureLimit: captureLimit, cancellation: cancellation)
        }.value
      } onCancel: {
        cancellation.cancel()
      }
    }
    private static func invoke(
      executable: String, arguments: [String], timeout: Duration, captureLimit: Int,
      cancellation: Cancellation
    ) throws -> String {
      guard !executable.isEmpty, !executable.hasPrefix("-"), timeout > .zero,
        (0...1024 * 1024).contains(captureLimit)
      else { throw ClientError("Choose the installed Zuse executable.") }
      guard !cancellation.isCancelled else { throw CancellationError() }
      let pipe = Pipe()
      var environment = ProcessInfo.processInfo.environment
      environment["PATH"] = [
        "/opt/homebrew/bin", "/usr/local/bin", NSHomeDirectory() + "/.local/bin",
        NSHomeDirectory() + "/.bun/bin", environment["PATH"] ?? "/usr/bin:/bin",
      ].joined(separator: ":")
      let descriptor = pipe.fileHandleForReading.fileDescriptor
      let flags = fcntl(descriptor, F_GETFL)
      guard flags != -1, fcntl(descriptor, F_SETFL, flags | O_NONBLOCK) != -1 else {
        throw ClientError("Could not read Zuse Serve status.")
      }
      defer {
        try? pipe.fileHandleForReading.close()
        try? pipe.fileHandleForWriting.close()
      }
      let writer = pipe.fileHandleForWriting.fileDescriptor
      #if os(macOS)
        var actions: posix_spawn_file_actions_t?
        var attributes: posix_spawnattr_t?
      #else
        var actions = posix_spawn_file_actions_t()
        var attributes = posix_spawnattr_t()
      #endif
      guard posix_spawn_file_actions_init(&actions) == 0 else {
        throw ClientError("Could not start Zuse Serve.")
      }
      defer { posix_spawn_file_actions_destroy(&actions) }
      guard posix_spawnattr_init(&attributes) == 0 else {
        throw ClientError("Could not start Zuse Serve.")
      }
      defer { posix_spawnattr_destroy(&attributes) }
      guard posix_spawn_file_actions_adddup2(&actions, writer, STDOUT_FILENO) == 0,
        posix_spawn_file_actions_adddup2(&actions, writer, STDERR_FILENO) == 0,
        posix_spawn_file_actions_addopen(&actions, STDIN_FILENO, "/dev/null", O_RDONLY, 0) == 0,
        posix_spawn_file_actions_addclose(&actions, descriptor) == 0,
        posix_spawn_file_actions_addclose(&actions, writer) == 0
      else { throw ClientError("Could not start Zuse Serve.") }
      // Do not let a service inherit unrelated app sockets or archive descriptors.
      #if os(macOS)
        guard posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_CLOEXEC_DEFAULT)) == 0 else {
          throw ClientError("Could not start Zuse Serve.")
        }
      #else
        guard posix_spawn_file_actions_addclosefrom_np(&actions, 3) == 0 else {
          throw ClientError("Could not start Zuse Serve.")
        }
      #endif
      let argv = (["/usr/bin/env", executable] + arguments).map { strdup($0) } + [nil]
      let envp = environment.map { strdup("\($0.key)=\($0.value)") } + [nil]
      defer {
        for entry in argv { free(entry) }
        for entry in envp { free(entry) }
      }
      var processID: pid_t = 0
      let launch = argv.withUnsafeBufferPointer { argv in
        envp.withUnsafeBufferPointer { envp in
          posix_spawn(
            &processID, "/usr/bin/env", &actions, &attributes, argv.baseAddress!, envp.baseAddress!)
        }
      }
      guard launch == 0 else {
        throw ClientError("Could not start Zuse Serve. Check its executable.")
      }
      try? pipe.fileHandleForWriting.close()
      var finished = false
      var exitStatus: Int32 = 0
      defer {
        if !finished {
          _ = kill(processID, SIGKILL)
          while waitpid(processID, &exitStatus, 0) < 0 && errno == EINTR {}
        }
      }
      let clock = ContinuousClock()
      let deadline = clock.now.advanced(by: timeout)
      var terminationDeadline: ContinuousClock.Instant?
      var timedOut = false
      var cancelled = false
      var captured = Data()
      var buffer = [UInt8](repeating: 0, count: 8192)
      func drain() {
        // Drain a bounded amount per iteration so continuous output cannot starve the deadline.
        for _ in 0..<8 {
          let count = buffer.withUnsafeMutableBytes { read(descriptor, $0.baseAddress, $0.count) }
          guard count > 0 else { break }
          if captured.count < captureLimit {
            captured.append(contentsOf: buffer.prefix(min(count, captureLimit - captured.count)))
          }
        }
      }
      while true {
        drain()
        let state = waitpid(processID, &exitStatus, WNOHANG)
        if state == processID {
          finished = true
          drain()
          break
        }
        if state < 0, errno != EINTR {
          if errno == ECHILD { finished = true }
          throw ClientError("Could not read Zuse Serve status.")
        }
        let now = clock.now
        cancelled = cancelled || cancellation.isCancelled
        timedOut = timedOut || now >= deadline
        if cancelled || timedOut {
          if let terminationDeadline {
            if now >= terminationDeadline { _ = kill(processID, SIGKILL) }
          } else {
            _ = kill(processID, SIGTERM)
            terminationDeadline = now.advanced(by: .milliseconds(250))
          }
        }
        var readiness = pollfd(fd: descriptor, events: Int16(POLLIN), revents: 0)
        _ = poll(&readiness, 1, 20)
        // EOF can arrive before process exit. Avoid spinning on a permanently readable pipe.
        if readiness.revents & Int16(POLLHUP) != 0 { Thread.sleep(forTimeInterval: 0.01) }
      }
      if cancelled || cancellation.isCancelled { throw CancellationError() }
      if timedOut {
        throw ClientError(
          "Zuse Serve did not respond in time. Refresh service status before trying again.")
      }
      guard exitStatus == 0 else {
        // CLI failures can include credentials; return no subprocess output in an error.
        throw ClientError(
          "Zuse Serve did not finish successfully. Check that Zuse is installed and run the command in Terminal for details."
        )
      }
      return String(decoding: captured, as: UTF8.self)
    }
  }
#endif
