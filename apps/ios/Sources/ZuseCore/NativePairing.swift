import Foundation

#if canImport(ZusePairing)
  import ZusePairing
#endif

#if canImport(Darwin)
  import Darwin
#elseif canImport(Glibc)
  import Glibc
#endif

/// Owns a loaded bridge for the lifetime of its function pointers. Rust serializes session state.
public final class NativePairing: @unchecked Sendable {
  private typealias Request = @convention(c) (UnsafePointer<UInt8>?, Int) -> UnsafeMutablePointer<
    CChar
  >?
  private typealias Release = @convention(c) (UnsafeMutablePointer<CChar>?) -> Void
  private let library: UnsafeMutableRawPointer?
  private let call: Request
  private let release: Release

  public init() throws {
    #if canImport(ZusePairing)
      // Direct references retain the static archive under Apple's dead stripping.
      library = nil
      call = zuse_mobile_request
      release = zuse_mobile_free
    #else
      let loaded: UnsafeMutableRawPointer?
      #if os(Linux)
        // Local integration tests supply the exact built library; Apple resolves linked symbols.
        if let path = ProcessInfo.processInfo.environment["ZUSE_NATIVE_PAIRING_LIBRARY"] {
          loaded = dlopen(path, RTLD_NOW | RTLD_LOCAL)
        } else {
          loaded = dlopen(nil, RTLD_NOW | RTLD_LOCAL)
        }
      #else
        loaded = dlopen(nil, RTLD_NOW | RTLD_LOCAL)
      #endif
      guard let loaded else { throw ClientError("Native pairing is unavailable in this build.") }
      guard let request = dlsym(loaded, "zuse_mobile_request"),
        let free = dlsym(loaded, "zuse_mobile_free")
      else {
        dlclose(loaded)
        throw ClientError("Native pairing is unavailable in this build.")
      }
      library = loaded
      call = unsafeBitCast(request, to: Request.self)
      release = unsafeBitCast(free, to: Release.self)
    #endif
  }
  deinit {
    if let library { dlclose(library) }
  }

  public func request(_ value: [String: JSONValue]) throws -> JSONValue {
    let bytes = try JSONEncoder().encode(JSONValue.object(value))
    guard bytes.count <= 24 * 1024 * 1024 else {
      throw ClientError("Pairing message exceeds the supported size.")
    }
    let result = bytes.withUnsafeBytes { raw in
      call(raw.bindMemory(to: UInt8.self).baseAddress, bytes.count)
    }
    guard let result else { throw ClientError("Native pairing is unavailable.") }
    defer { release(result) }
    let decoded = try JSONDecoder().decode(JSONValue.self, from: Data(String(cString: result).utf8))
    guard decoded["error"].string == nil else {
      throw ClientError("Paired connection unavailable. Reconnect with a valid invitation.")
    }
    return decoded
  }
}
