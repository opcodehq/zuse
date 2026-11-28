import Foundation
import Testing

@testable import ZuseCore

@Test func directoryPagesDecodeFromTheEngineShape() throws {
  let page = try JSONDecoder().decode(
    WorkspaceDirectoryPage.self,
    from: Data(
      """
      {"checkoutId":"folder-1","mutationCapabilities":{"moveEntry":true,"deleteEntry":true},
       "directory":"","truncated":false,"nextCursor":"c2",
       "entries":[
        {"mutationRevision":"r1","path":"src","name":"src","kind":"directory",
         "modifiedAt":"2026-11-28T06:23:49.403Z","ignored":false,"readOnly":true},
        {"path":"README.md","name":"README.md","kind":"file","size":52,"ignored":false,
         "readOnly":false},
        {"path":"link","kind":"symlink","ignored":true,"readOnly":true}
       ]}
      """.utf8))
  #expect(page.nextCursor == "c2")
  #expect(page.entries.map(\.path) == ["src", "README.md", "link"])
  #expect(page.entries[0].isDirectory)
  #expect(page.entries[1].size == 52)
  #expect(page.entries[2].name == "link")
  #expect(page.entries[2].ignored)
}

@Test func fileSearchMatchesDecodeAsEntries() throws {
  let matches = try JSONValue.array([
    .object([
      "path": .string("src/app/index.ts"), "name": .string("index.ts"), "kind": .string("file"),
      "score": .number(90),
    ])
  ]).decode([WorkspaceFileEntry].self)
  #expect(matches.first?.path == "src/app/index.ts")
  #expect(matches.first?.isDirectory == false)
}

@Test func fileTreeShowsOpenFoldersFirstAndIndented() {
  let children: [String: [WorkspaceFileEntry]] = [
    "": FileTree.sorted([
      WorkspaceFileEntry(path: "b.txt", kind: .file),
      WorkspaceFileEntry(path: "src", kind: .directory),
      WorkspaceFileEntry(path: "A.md", kind: .file),
      WorkspaceFileEntry(path: ".github", kind: .directory),
    ]),
    "src": [
      WorkspaceFileEntry(path: "src/app", kind: .directory),
      WorkspaceFileEntry(path: "src/main.rs", kind: .file),
    ],
    "src/app": [WorkspaceFileEntry(path: "src/app/index.ts", kind: .file)],
  ]
  #expect(children[""]?.map(\.name) == [".github", "src", "A.md", "b.txt"])
  let closed = FileTree.rows(children: children, expanded: [])
  #expect(closed.map(\.id) == [".github", "src", "A.md", "b.txt"])
  // A folder inside a closed folder stays hidden even when it is open itself.
  let nested = FileTree.rows(children: children, expanded: ["src/app"])
  #expect(nested.count == 4)
  let open = FileTree.rows(children: children, expanded: ["src", "src/app"])
  #expect(
    open.map(\.id) == [
      ".github", "src", "src/app", "src/app/index.ts", "src/main.rs", "A.md", "b.txt",
    ])
  #expect(open.map(\.depth) == [0, 0, 1, 2, 1, 0, 0])
}

@Test func terminalSessionsAndStreamEventsDecode() throws {
  let session = try JSONValue.object([
    "id": .string("t1"), "cwd": .string("/tmp/repo"), "shell": .string("zsh"),
  ]).decode(TerminalSessionInfo.self)
  #expect(session == TerminalSessionInfo(id: "t1", cwd: "/tmp/repo", shell: "zsh"))
  let data = TerminalStreamEvent(
    .object([
      "type": .string("data"), "seq": .number(3),
      "data": .string(Data("hi\r\n".utf8).base64EncodedString()),
    ]))
  #expect(data == .data(seq: 3, bytes: Array("hi\r\n".utf8)))
  #expect(data?.seq == 3)
  let exit = TerminalStreamEvent(
    .object(["type": .string("exit"), "seq": .number(4), "exitCode": .number(130)]))
  #expect(exit == .exit(seq: 4, code: 130, signal: nil))
  #expect(TerminalStreamEvent(.object(["type": .string("data"), "seq": .number(1)])) == nil)
  #expect(TerminalStreamEvent(.object(["type": .string("resize"), "seq": .number(1)])) == nil)
  #expect(TerminalStreamEvent(.object(["type": .string("data"), "seq": .number(-1)])) == nil)
}
