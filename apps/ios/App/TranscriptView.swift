import SwiftUI
import Textual
import ZuseCore

#if os(iOS)
  import UIKit
#endif

#if os(iOS)
  /// UIKit owns recycling and scroll position; SwiftUI only renders visible message cells.
  struct TranscriptView: UIViewControllerRepresentable {
    let messages: [ChatMessage]
    let sessionID: String
    let rpc: RPCClient?
    var bottomInset: CGFloat = 0
    var respond: ((ChatMessage, String) async throws -> Void)?
    func makeUIViewController(context: Context) -> TranscriptController { TranscriptController() }
    func updateUIViewController(_ controller: TranscriptController, context: Context) {
      controller.setBottomInset(bottomInset)
      controller.respond = respond
      controller.setConnection(rpc)
      controller.update(messages, sessionID: sessionID)
    }
  }

  @MainActor final class TranscriptController: UIViewController, UICollectionViewDelegate {
    var respond: ((ChatMessage, String) async throws -> Void)?
    private var rpc: RPCClient?
    private var connectionChanged = false
    func setConnection(_ value: RPCClient?) {
      if rpc !== value {
        rpc = value
        connectionChanged = true
      }
    }
    private var collection: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<Int, String>!
    private var rows: [String: TranscriptGroup] = [:]
    private var sessionID = ""
    private var applying = false
    private var queued: ([ChatMessage], String)?
    override func viewDidLoad() {
      super.viewDidLoad()
      let size = NSCollectionLayoutSize(
        widthDimension: .fractionalWidth(1), heightDimension: .estimated(36))
      let item = NSCollectionLayoutItem(layoutSize: size)
      let group = NSCollectionLayoutGroup.vertical(layoutSize: size, subitems: [item])
      let section = NSCollectionLayoutSection(group: group)
      collection = UICollectionView(
        frame: .zero,
        collectionViewLayout: UICollectionViewCompositionalLayout(section: section))
      collection.selfSizingInvalidation = .enabledIncludingConstraints
      collection.accessibilityIdentifier = "chat-transcript"
      view.backgroundColor = AppTheme.background
      collection.backgroundColor = .clear
      collection.keyboardDismissMode = .interactive
      collection.delegate = self
      collection.allowsSelection = false
      collection.alwaysBounceVertical = true
      collection.translatesAutoresizingMaskIntoConstraints = false
      view.addSubview(collection)
      NSLayoutConstraint.activate([
        collection.leadingAnchor.constraint(equalTo: view.leadingAnchor),
        collection.trailingAnchor.constraint(equalTo: view.trailingAnchor),
        collection.topAnchor.constraint(equalTo: view.topAnchor),
        collection.bottomAnchor.constraint(equalTo: view.bottomAnchor),
      ])
      let registration = UICollectionView.CellRegistration<UICollectionViewCell, String> {
        [weak self] cell, _, id in
        guard let row = self?.rows[id] else { return }
        cell.contentConfiguration = UIHostingConfiguration {
          Group {
            if row.isActivity {
              ToolActivityGroup(
                group: row,
                toggleDisclosure: { [weak self] change in
                  self?.resizeDisclosure(change)
                })
            } else {
              MessageCell(
                message: row.rows[0].message, rpc: self?.rpc,
                respond: self?.respond.map { callback in
                  { text in try await callback(row.rows[0].message, text) }
                })
            }
          }.id(row.id)
        }.margins(
          .all, 0
        ).minSize(width: 0, height: 0)
        cell.backgroundConfiguration = .clear()
      }
      dataSource = UICollectionViewDiffableDataSource<Int, String>(collectionView: collection) {
        view, index, id in
        view.dequeueConfiguredReusableCell(using: registration, for: index, item: id)
      }
    }
    func scrollViewWillBeginDragging(_ scrollView: UIScrollView) {
      view.window?.endEditing(true)
    }
    private var previousViewportHeight: CGFloat = 0
    override func viewDidLayoutSubviews() {
      super.viewDidLayoutSubviews()
      let height = collection.bounds.height
      defer { previousViewportHeight = height }
      guard previousViewportHeight > 0, abs(height - previousViewportHeight) > 1,
        !collection.isDragging, !collection.isDecelerating
      else { return }
      let previousDistance =
        collection.contentSize.height - collection.contentOffset.y
        - previousViewportHeight + collection.adjustedContentInset.bottom
      if previousDistance < 100 {
        scrollToLatest(sessionID: sessionID, remainingPasses: 0)
      }
    }
    func setBottomInset(_ height: CGFloat) {
      loadViewIfNeeded()
      guard abs(collection.contentInset.bottom - height) > 0.5 else { return }
      let atBottom =
        collection.contentSize.height - collection.bounds.maxY
        + collection.adjustedContentInset.bottom < 40
      collection.contentInset.bottom = height
      collection.verticalScrollIndicatorInsets.bottom = height
      if atBottom && !collection.isDragging && !collection.isDecelerating {
        let y = max(
          -collection.adjustedContentInset.top,
          collection.contentSize.height - collection.bounds.height
            + collection.adjustedContentInset.bottom)
        collection.setContentOffset(CGPoint(x: 0, y: y), animated: false)
      }
    }
    private func resizeDisclosure(_ change: () -> Void) {
      // The collection owns cell geometry. Avoid competing SwiftUI and UIKit height animations.
      UIView.performWithoutAnimation {
        change()
        collection.collectionViewLayout.invalidateLayout()
        collection.layoutIfNeeded()
      }
      DispatchQueue.main.async { [weak self] in
        guard let self else { return }
        UIView.performWithoutAnimation {
          self.collection.collectionViewLayout.invalidateLayout()
          self.collection.layoutIfNeeded()
        }
      }
    }
    func update(_ messages: [ChatMessage], sessionID: String) {
      loadViewIfNeeded()
      if applying {
        queued = (messages, sessionID)
        return
      }
      let changedSession = self.sessionID != sessionID
      let prior = dataSource.snapshot().itemIdentifiers
      let presented = TranscriptGroup.project(messages)
      let incoming = presented.map(\.id)
      let changed = presented.filter { connectionChanged || rows[$0.id] != $0 }.map(\.id)
      connectionChanged = false
      guard changedSession || incoming != prior || !changed.isEmpty else { return }
      let atBottom =
        collection.contentSize.height - collection.bounds.maxY
        + collection.adjustedContentInset.bottom < 100
      let anchor = collection.indexPathsForVisibleItems.sorted().first
      let anchorID = anchor.flatMap { dataSource.itemIdentifier(for: $0) }
      let offset = anchor.flatMap { collection.layoutAttributesForItem(at: $0)?.frame.minY }.map {
        $0 - collection.contentOffset.y
      }
      self.sessionID = sessionID
      rows = Dictionary(uniqueKeysWithValues: presented.map { ($0.id, $0) })
      var snapshot = NSDiffableDataSourceSnapshot<Int, String>()
      snapshot.appendSections([0])
      snapshot.appendItems(incoming)
      let existing = Set(prior)
      snapshot.reconfigureItems(changed.filter { existing.contains($0) })
      applying = true
      dataSource.apply(snapshot, animatingDifferences: false) { [weak self] in
        guard let self else { return }
        collection.layoutIfNeeded()
        if changedSession || atBottom, !incoming.isEmpty {
          scrollToLatest(sessionID: sessionID, remainingPasses: 3)
        } else if let anchorID, let offset, let path = dataSource.indexPath(for: anchorID),
          let attributes = collection.layoutAttributesForItem(at: path)
        {
          collection.contentOffset.y = attributes.frame.minY - offset
        }
        applying = false
        if let queued {
          self.queued = nil
          update(queued.0, sessionID: queued.1)
        }
      }
    }
    private func scrollToLatest(sessionID: String, remainingPasses: Int) {
      guard self.sessionID == sessionID, let last = dataSource.snapshot().itemIdentifiers.last,
        let path = dataSource.indexPath(for: last), !collection.isDragging,
        !collection.isDecelerating
      else { return }
      collection.layoutIfNeeded()
      collection.scrollToItem(at: path, at: .bottom, animated: false)
      if remainingPasses > 0 {
        DispatchQueue.main.async { [weak self] in
          self?.scrollToLatest(sessionID: sessionID, remainingPasses: remainingPasses - 1)
        }
      }
    }
  }

#else
  import AppKit

  /// AppKit recycles hosting cells and estimates offscreen row heights.
  struct TranscriptView: NSViewControllerRepresentable {
    let messages: [ChatMessage]
    let sessionID: String
    let rpc: RPCClient?
    var bottomInset: CGFloat = 0
    var respond: ((ChatMessage, String) async throws -> Void)?
    func makeNSViewController(context: Context) -> MacTranscriptController {
      MacTranscriptController()
    }
    func updateNSViewController(_ controller: MacTranscriptController, context: Context) {
      controller.update(messages, sessionID: sessionID, rpc: rpc, respond: respond)
    }
  }

  @MainActor
  final class MacTranscriptController: NSViewController,
    NSTableViewDataSource, NSTableViewDelegate
  {
    private let table = NSTableView()
    private let scroll = NSScrollView()
    private let latest = NSButton(title: "Latest", target: nil, action: nil)
    private var messages: [ChatMessage] = []
    private var sessionID = ""
    private var heights: [String: CGFloat] = [:]
    private var measuredWidth: CGFloat = 0
    private var rpc: RPCClient?
    private var respond: ((ChatMessage, String) async throws -> Void)?
    override func loadView() {
      view = NSView()
      let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("message"))
      column.resizingMask = .autoresizingMask
      table.addTableColumn(column)
      table.headerView = nil
      table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
      table.backgroundColor = .clear
      table.selectionHighlightStyle = .none
      table.intercellSpacing = .zero
      table.rowHeight = 160
      table.usesAutomaticRowHeights = false
      table.dataSource = self
      table.delegate = self
      table.setAccessibilityIdentifier("chat-transcript")
      scroll.documentView = table
      scroll.hasVerticalScroller = true
      scroll.drawsBackground = false
      scroll.translatesAutoresizingMaskIntoConstraints = false
      view.addSubview(scroll)
      latest.target = self
      latest.action = #selector(showLatest)
      latest.bezelStyle = .rounded
      latest.translatesAutoresizingMaskIntoConstraints = false
      latest.isHidden = true
      view.addSubview(latest)
      NSLayoutConstraint.activate([
        scroll.leadingAnchor.constraint(equalTo: view.leadingAnchor),
        scroll.trailingAnchor.constraint(equalTo: view.trailingAnchor),
        scroll.topAnchor.constraint(equalTo: view.topAnchor),
        scroll.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        latest.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -20),
        latest.bottomAnchor.constraint(equalTo: view.bottomAnchor, constant: -12),
      ])
      scroll.contentView.postsBoundsChangedNotifications = true
      NotificationCenter.default.addObserver(
        self, selector: #selector(boundsChanged),
        name: NSView.boundsDidChangeNotification, object: scroll.contentView)
    }
    @objc private func boundsChanged() { updateLatestButton() }
    func numberOfRows(in tableView: NSTableView) -> Int { messages.count }
    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }
    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
      heights[messages[row].id] ?? (messages[row].role == "user" ? 64 : 240)
    }
    func tableViewColumnDidResize(_ notification: Notification) {
      guard abs(measuredWidth - table.bounds.width) > 1 else { return }
      measuredWidth = table.bounds.width
      heights.removeAll()
      table.noteHeightOfRows(withIndexesChanged: IndexSet(integersIn: messages.indices))
    }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int)
      -> NSView?
    {
      let id = NSUserInterfaceItemIdentifier("message-cell")
      let cell =
        table.makeView(withIdentifier: id, owner: nil) as? MacMessageCell ?? MacMessageCell()
      cell.identifier = id
      let message = messages[row]
      cell.set(
        MessageCell(
          message: message, rpc: rpc,
          respond: respond.map { callback in { text in try await callback(message, text) } }))
      DispatchQueue.main.async { [weak self, weak cell] in
        guard let self, let cell,
          let index = self.messages.firstIndex(where: { $0.id == message.id }),
          self.table.row(for: cell) == index
        else { return }
        let follow = self.atBottom
        let height = max(32, cell.measuredHeight(width: self.table.bounds.width))
        guard abs((self.heights[message.id] ?? 0) - height) > 1 else { return }
        self.heights[message.id] = height
        self.table.noteHeightOfRows(withIndexesChanged: IndexSet(integer: index))
        if follow { self.table.scrollRowToVisible(self.messages.count - 1) }
      }
      return cell
    }
    func update(
      _ incoming: [ChatMessage], sessionID: String, rpc: RPCClient?,
      respond: ((ChatMessage, String) async throws -> Void)?
    ) {
      _ = view
      let switched = self.sessionID != sessionID
      let follow = switched || atBottom
      let visible = table.rows(in: scroll.documentVisibleRect)
      let anchor = visible.location < messages.count ? messages[visible.location].id : nil
      let offset =
        visible.location < messages.count
        ? table.rect(ofRow: visible.location).minY - scroll.contentView.bounds.minY : 0
      let structureChanged = switched || messages.map(\.id) != incoming.map(\.id)
      let changed = IndexSet(
        incoming.indices.filter {
          self.rpc !== rpc || $0 >= messages.count || messages[$0] != incoming[$0]
        })
      self.sessionID = sessionID
      self.rpc = rpc
      self.respond = respond
      if switched { heights.removeAll() }
      for index in changed { heights.removeValue(forKey: incoming[index].id) }
      messages = incoming
      if structureChanged {
        table.reloadData()
      } else if !changed.isEmpty {
        table.reloadData(forRowIndexes: changed, columnIndexes: IndexSet(integer: 0))
        table.noteHeightOfRows(withIndexesChanged: changed)
      } else {
        return
      }
      view.layoutSubtreeIfNeeded()
      if follow {
        scrollToLatest(sessionID: sessionID, passes: 3)
      } else if let anchor, let row = messages.firstIndex(where: { $0.id == anchor }) {
        scroll.contentView.scroll(to: NSPoint(x: 0, y: table.rect(ofRow: row).minY - offset))
        scroll.reflectScrolledClipView(scroll.contentView)
      }
      updateLatestButton()
    }
    private var atBottom: Bool {
      table.bounds.height - scroll.documentVisibleRect.maxY < 100
    }
    private func updateLatestButton() { latest.isHidden = messages.isEmpty || atBottom }
    @objc private func showLatest() { scrollToLatest(sessionID: sessionID, passes: 3) }
    private func scrollToLatest(sessionID: String, passes: Int) {
      guard self.sessionID == sessionID, !messages.isEmpty else { return }
      table.scrollRowToVisible(messages.count - 1)
      updateLatestButton()
      if passes > 0 {
        DispatchQueue.main.async { [weak self] in
          self?.scrollToLatest(sessionID: sessionID, passes: passes - 1)
        }
      }
    }
  }

  @MainActor private final class MacMessageCell: NSTableCellView {
    private var hosting: NSHostingView<MessageCell>?
    func measuredHeight(width: CGFloat) -> CGFloat {
      frame.size.width = width
      layoutSubtreeIfNeeded()
      return hosting?.fittingSize.height ?? 160
    }
    func set(_ content: MessageCell) {
      if let hosting {
        hosting.rootView = content
        return
      }
      let hosting = NSHostingView(rootView: content)
      hosting.translatesAutoresizingMaskIntoConstraints = false
      addSubview(hosting)
      NSLayoutConstraint.activate([
        hosting.leadingAnchor.constraint(equalTo: leadingAnchor),
        hosting.trailingAnchor.constraint(equalTo: trailingAnchor),
        hosting.topAnchor.constraint(equalTo: topAnchor),
        hosting.bottomAnchor.constraint(equalTo: bottomAnchor),
      ])
      self.hosting = hosting
    }
  }

#endif

struct MessageCell: View {
  let message: ChatMessage
  let rpc: RPCClient?
  var respond: ((String) async throws -> Void)?
  private var isUser: Bool { message.role == "user" }
  var body: some View {
    VStack(alignment: .leading, spacing: 5) {
      if ["assistant", "user", "user_rich"].contains(message.kind) {
        ChatMarkdown(text: message.text)
          .frame(maxWidth: .infinity, alignment: .leading)
        if message.kind == "user_rich", !message.content["attachments"].array.isEmpty {
          ForEach(Array(message.content["attachments"].array.enumerated()), id: \.offset) {
            _, reference in
            AttachmentView(reference: reference, sessionID: message.sessionId, rpc: rpc)
          }
        }
      } else {
        MessageDetails(message: message, respond: respond)
          .disclosureGroupStyle(TranscriptDisclosureStyle())
      }
    }
    .font(.subheadline)
    .padding(isUser ? 12 : 0)
    .background(
      isUser ? Color.secondary.opacity(0.07) : .clear, in: RoundedRectangle(cornerRadius: 16)
    )
    .padding(.horizontal, 16).padding(
      .vertical, isUser ? 10 : (message.kind == "assistant" ? 7 : 3)
    )
    .frame(maxWidth: 720, alignment: .leading).frame(maxWidth: .infinity)
    .contextMenu {
      Button("Copy", systemImage: "doc.on.doc") { NativePlatform.copy(message.text) }
      ShareLink(item: message.text)
    }
  }
  private var title: String {
    message.content["tool"].string
      ?? message.kind.replacingOccurrences(of: "_", with: " ").capitalized
  }
  private var icon: String {
    message.kind == "thinking"
      ? "brain" : message.kind == "error" ? "exclamationmark.triangle" : "terminal"
  }
}
