#if DEBUG
  import Foundation
  import ZuseCore

  /// `--cloud-preview`: a signed-in account whose cloud API answers from fixtures shaped like
  /// the Rust routes, for screenshots and manual checks without a real account.
  enum CloudPreview {
    static var enabled: Bool { ProcessInfo.processInfo.arguments.contains("--cloud-preview") }
    static let baseURL = URL(string: "https://cloud-preview.zuse.invalid")!
    /// An unsigned token that only carries the claims the app reads.
    static var credentials: Data {
      let claims = Data(
        #"{"sub":"user_preview","email":"preview@zuse.invalid","exp":4102444800}"#.utf8
      )
      .base64EncodedString().replacingOccurrences(of: "=", with: "")
      .replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_")
      return Data(
        #"{"access_token":"e30.\#(claims).preview","refresh_token":"preview","user":{"first_name":"Preview","last_name":"User","email":"preview@zuse.invalid","profile_picture_url":"https://cloud-preview.zuse.invalid/avatar.png"}}"#
          .utf8)
    }
    static func client() -> CloudClient {
      let configuration = URLSessionConfiguration.ephemeral
      configuration.protocolClasses = [CloudPreviewProtocol.self]
      return CloudClient(baseURL: baseURL, session: URLSession(configuration: configuration)) {
        "preview"
      }
    }
    static let catalog =
      #"{"providers":[{"id":"claude-code","name":"Claude","defaultModel":"sonnet","models":[{"id":"claude-sonnet-preview","label":"Sonnet","reasoningLevels":["low","medium","high","max"]},{"id":"claude-opus-preview","label":"Opus","reasoningLevels":["low","medium","high","xhigh","max"]}]},{"id":"codex","name":"Codex","defaultModel":"codex-preview","models":[{"id":"codex-preview","label":"Codex","reasoningLevels":["low","medium","high"]},{"id":"codex-mini-preview","label":"Codex mini"}]},{"id":"grok","name":"Grok","defaultModel":"grok-preview","models":[{"id":"grok-preview","label":"Grok"}]}]}"#
    static let capabilities =
      #"{"providers":[{"id":"e2b","configured":true,"managedBaseReady":true,"commandAdmissionVersion":2,"preservesDisk":true,"preservesMemory":true,"supportsSnapshots":true,"supportsDocker":false,"sizes":[{"id":"default","label":"Standard","vcpus":4,"memoryMib":16384,"diskGib":40}],"unavailableReason":null}],"canConfigure":true,"canAllocate":true,"allocationBlockers":[],"defaultProvider":"e2b","computerSizes":[{"id":"default","label":"Standard · 4 vCPU · 16 GB","vcpus":4,"memoryMib":16384,"machineRate":4,"available":true},{"id":"large","label":"Large · 8 vCPU · 32 GB","vcpus":8,"memoryMib":32768,"machineRate":8,"available":false,"requiresPlan":"cloud-max"}]}"#
    static let connections =
      #"[{"connectionId":"conn_codex","agent":"codex","method":"device-code","state":"connected","accountLabel":"preview@zuse.invalid","expiresAt":null,"version":3,"errorCode":null,"connectionScope":"personal"},{"connectionId":"conn_claude","agent":"claude","method":"setup-token","state":"reconnect-required","accountLabel":"Claude subscription","expiresAt":null,"version":2,"errorCode":null,"connectionScope":"personal"}]"#
    static let projects =
      #"[{"projectId":"proj_web","repositoryIdentity":"github.com/acme/web","repositoryUrl":"https://github.com/acme/web","displayName":"acme/web","defaultBranch":"main","visibility":"private","state":"ready","activeBuilds":{},"latestBuilds":{},"createdAt":0,"updatedAt":0},{"projectId":"proj_api","repositoryIdentity":"github.com/acme/api","repositoryUrl":"https://github.com/acme/api","displayName":"acme/api","defaultBranch":"develop","visibility":"private","state":"ready","activeBuilds":{},"latestBuilds":{},"createdAt":0,"updatedAt":0}]"#
    static func response(_ request: URLRequest) -> (Int, String) {
      let path = request.url?.path ?? ""
      switch (request.httpMethod ?? "GET", path) {
      case ("GET", "/v1/cloud/context"):
        return (
          200,
          #"{"actor":{"accountId":"user_preview"},"scope":{"kind":"personal"},"ownerId":"user_preview","accountEvents":true,"projects":\#(projects),"agentCatalog":\#(catalog),"providerAgentCatalogs":{"e2b":\#(catalog)},"computerCapabilities":\#(capabilities),"commandAdmissionVersion":2,"launchAttachmentHoldVersion":1,"agentConnections":\#(connections)}"#
        )
      case ("GET", "/v1/cloud/capabilities"): return (200, capabilities)
      case ("GET", "/v1/cloud/projects"): return (200, #"{"projects":\#(projects)}"#)
      case ("GET", "/v1/cloud/agents"):
        return (
          200,
          #"{"connections":\#(connections),"supportsDeviceReconnect":true,"supportsCredentialReconfiguration":true}"#
        )
      case ("GET", "/v1/cloud/workspaces"): return (200, #"{"workspaces":[]}"#)
      case ("GET", "/v1/environments"): return (200, #"{"environments":[]}"#)
      default: return (409, #"{"error":"cloud_workspace_creation_disabled"}"#)
      }
    }
  }

  final class CloudPreviewProtocol: URLProtocol, @unchecked Sendable {
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
      guard let url = request.url else { return }
      let (status, body) = CloudPreview.response(request)
      let response = HTTPURLResponse(
        url: url, statusCode: status, httpVersion: "HTTP/1.1",
        headerFields: ["Content-Type": "application/json"])!
      client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
      client?.urlProtocol(self, didLoad: Data(body.utf8))
      client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
  }
#endif
