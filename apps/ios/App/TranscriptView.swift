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
    /// The host reports a turn in progress.
    var running = false
    var bottomInset: CGFloat = 0
    var respond: ((ChatMessage, String) async throws -> Void)?
    /// Opens the computer's working changes from a turn's changes card.
    var openChanges: (() -> Void)?
    func makeUIViewController(context: Context) -> TranscriptController { TranscriptController() }
    func updateUIViewController(_ controller: TranscriptController, context: Context) {
      controller.setBottomInset(bottomInset)
      controller.respond = respond
      controller.setOpenChanges(openChanges)
      controller.setConnection(rpc)
      controller.update(messages, sessionID: sessionID, running: running)
    }
  }

  @MainActor final class TranscriptController: UIViewController, UICollectionViewDelegate {
    var respond: ((ChatMessage, String) async throws -> Void)?
    private var rpc: RPCClient?
    private var connectionChanged = false
    private var openChanges: (() -> Void)?
    func setConnection(_ value: RPCClient?) {
      if rpc !== value {
        rpc = value
        connectionChanged = true
      }
    }
    /// Cells capture the action when configured, so gaining or losing it reconfigures them.
    func setOpenChanges(_ value: (() -> Void)?) {
      if (openChanges == nil) != (value == nil) { connectionChanged = true }
      openChanges = value
    }
    private var collection: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<Int, String>!
    private var rows: [String: TimelineItem] = [:]
    private var sessionID = ""
    private var applying = false
    private var queued: ([ChatMessage], String, Bool)?
    override func viewDidLoad() {
      super.viewDidLoad()
      let size = NSCollectionLayoutSize(
        widthDimension: .fractionalWidth(1), heightDimension: .estimated(60))
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
          TimelineItemView(
            item: row, rpc: self?.rpc, respond: self?.respond,
            resize: { [weak self] change in self?.resizeDisclosure(change) },
            openChanges: self?.openChanges == nil ? nil : { [weak self] in self?.openChanges?() }
          )
          .id(row.id)
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
    private var measuredWidth: CGFloat = 0
    /// Re-measuring waits for a snapshot in flight, then runs once it lands.
    private enum Remeasure: Int, Comparable {
      case none, visible, all
      static func < (lhs: Self, rhs: Self) -> Bool { lhs.rawValue < rhs.rawValue }
    }
    private var pendingRemeasure = Remeasure.none
    override func viewDidAppear(_ animated: Bool) {
      super.viewDidAppear(animated)
      // Rows laid out while the chat was hidden (under another page, or while a split view
      // column was resizing) carry heights for another width.
      remeasure(.all)
    }
    /// Self-sized hosting cells keep the height they had at the width they were measured at, and
    /// the layout stacks rows by those heights. After the column changes width every row has to
    /// measure again, or rows overlap; after the viewport changes height, the visible rows do.
    private func remeasure(_ scope: Remeasure) {
      guard dataSource != nil, scope != .none else { return }
      if applying {
        pendingRemeasure = max(pendingRemeasure, scope)
        return
      }
      let snapshot = dataSource.snapshot()
      guard !snapshot.itemIdentifiers.isEmpty else { return }
      let position = savePosition()
      applying = true
      let done: () -> Void = { [weak self] in
        guard let self else { return }
        collection.collectionViewLayout.invalidateLayout()
        collection.layoutIfNeeded()
        restore(position)
        finishApply()
      }
      if scope == .all {
        dataSource.applySnapshotUsingReloadData(snapshot, completion: done)
      } else {
        var reconfigured = snapshot
        reconfigured.reconfigureItems(
          collection.indexPathsForVisibleItems.compactMap { dataSource.itemIdentifier(for: $0) })
        dataSource.apply(reconfigured, animatingDifferences: false, completion: done)
      }
    }
    private struct Position {
      let atBottom: Bool
      let anchorID: String?
      let offset: CGFloat?
    }
    private var isAtBottom: Bool {
      collection.contentSize.height - collection.bounds.maxY
        + collection.adjustedContentInset.bottom < 100
    }
    private func savePosition() -> Position {
      let anchor = collection.indexPathsForVisibleItems.sorted().first
      return Position(
        atBottom: isAtBottom,
        anchorID: anchor.flatMap { dataSource.itemIdentifier(for: $0) },
        offset: anchor.flatMap { collection.layoutAttributesForItem(at: $0)?.frame.minY }.map {
          $0 - collection.contentOffset.y
        })
    }
    private func restore(_ position: Position) {
      if position.atBottom, !TranscriptPreviewFlags.startAtTop {
        scrollToLatest(sessionID: sessionID, remainingPasses: 2)
      } else if let id = position.anchorID, let offset = position.offset,
        let path = dataSource.indexPath(for: id),
        let attributes = collection.layoutAttributesForItem(at: path)
      {
        collection.contentOffset.y = attributes.frame.minY - offset
      }
    }
    /// Ends a snapshot apply, then runs whatever waited for it.
    private func finishApply() {
      applying = false
      if let queued {
        self.queued = nil
        update(queued.0, sessionID: queued.1, running: queued.2)
      }
      if !applying, pendingRemeasure != .none {
        let scope = pendingRemeasure
        pendingRemeasure = .none
        remeasure(scope)
      }
    }
    override func viewDidLayoutSubviews() {
      super.viewDidLayoutSubviews()
      let height = collection.bounds.height
      defer { previousViewportHeight = height }
      // Rows measured while the chat was off screen or mid-transition can carry another width.
      if abs(collection.bounds.width - measuredWidth) > 1, collection.bounds.width > 0 {
        let first = measuredWidth == 0
        measuredWidth = collection.bounds.width
        if !first { remeasure(.all) }
      }
      guard previousViewportHeight > 0, abs(height - previousViewportHeight) > 1 else { return }
      // A new viewport (the tab bar hiding, the keyboard) can leave self-sized rows at stale
      // positions; re-measuring the visible rows puts them back in place.
      remeasure(.visible)
      guard !collection.isDragging, !collection.isDecelerating else { return }
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
    func update(_ messages: [ChatMessage], sessionID: String, running: Bool) {
      loadViewIfNeeded()
      if applying {
        queued = (messages, sessionID, running)
        return
      }
      let changedSession = self.sessionID != sessionID
      let prior = dataSource.snapshot().itemIdentifiers
      let presented = ChatTimeline.project(messages, running: running)
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
        if changedSession || atBottom, !incoming.isEmpty, !TranscriptPreviewFlags.startAtTop {
          scrollToLatest(sessionID: sessionID, remainingPasses: 3)
        } else if let anchorID, let offset, let path = dataSource.indexPath(for: anchorID),
          let attributes = collection.layoutAttributesForItem(at: path)
        {
          collection.contentOffset.y = attributes.frame.minY - offset
        }
        finishApply()
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
    /// The host reports a turn in progress.
    var running = false
    var bottomInset: CGFloat = 0
    var respond: ((ChatMessage, String) async throws -> Void)?
    var openChanges: (() -> Void)?
    func makeNSViewController(context: Context) -> MacTranscriptController {
      MacTranscriptController()
    }
    func updateNSViewController(_ controller: MacTranscriptController, context: Context) {
      controller.update(
        ChatTimeline.project(messages, running: running), sessionID: sessionID, rpc: rpc,
        respond: respond, openChanges: openChanges)
    }
  }

  @MainActor
  final class MacTranscriptController: NSViewController,
    NSTableViewDataSource, NSTableViewDelegate
  {
    private let table = NSTableView()
    private let scroll = NSScrollView()
    private let latest = NSButton(title: "Latest", target: nil, action: nil)
    private var messages: [TimelineItem] = []
    private var sessionID = ""
    private var heights: [String: CGFloat] = [:]
    private var measuredWidth: CGFloat = 0
    private var rpc: RPCClient?
    private var respond: ((ChatMessage, String) async throws -> Void)?
    private var openChanges: (() -> Void)?
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
      heights[messages[row].id] ?? 120
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
      let item = messages[row]
      cell.set(
        TimelineItemView(
          item: item, rpc: rpc, respond: respond,
          resize: { [weak self] change in
            change()
            DispatchQueue.main.async { self?.measure(item.id) }
          }, openChanges: openChanges))
      DispatchQueue.main.async { [weak self] in self?.measure(item.id) }
      return cell
    }
    /// Sizes a visible row to its content after it renders or changes.
    private func measure(_ id: String) {
      guard let index = messages.firstIndex(where: { $0.id == id }),
        let cell = table.view(atColumn: 0, row: index, makeIfNecessary: false) as? MacMessageCell
      else { return }
      let follow = atBottom
      let height = max(24, cell.measuredHeight(width: table.bounds.width))
      guard abs((heights[id] ?? 0) - height) > 1 else { return }
      heights[id] = height
      table.noteHeightOfRows(withIndexesChanged: IndexSet(integer: index))
      if follow { table.scrollRowToVisible(messages.count - 1) }
    }
    func update(
      _ incoming: [TimelineItem], sessionID: String, rpc: RPCClient?,
      respond: ((ChatMessage, String) async throws -> Void)?,
      openChanges: (() -> Void)? = nil
    ) {
      _ = view
      let actionChanged = (self.openChanges == nil) != (openChanges == nil)
      self.openChanges = openChanges
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
          actionChanged || self.rpc !== rpc || $0 >= messages.count || messages[$0] != incoming[$0]
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
    private var hosting: NSHostingView<TimelineItemView>?
    func measuredHeight(width: CGFloat) -> CGFloat {
      frame.size.width = width
      layoutSubtreeIfNeeded()
      return hosting?.fittingSize.height ?? 160
    }
    func set(_ content: TimelineItemView) {
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
