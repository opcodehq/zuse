import SwiftUI
import ZuseCore

/// A chat's pages side by side: swipe between them full screen, or pinch (or press the header
/// button) to zoom out to cards. In the overview a card swipes up to close and a tap brings it
/// back; the chat and the picker can't be closed.
struct ChatPanePager<Page: View>: View {
  let store: ChatPaneStore
  let chatID: String
  /// The label under a card in the overview.
  let label: (ChatPanePage) -> (title: String, symbol: String)?
  @ViewBuilder let page: (ChatPanePage) -> Page

  @State private var scroll = ScrollPosition(idType: String.self)
  /// The fingers' magnification while pinching.
  @State private var magnification: CGFloat?
  @State private var lift: (id: String, offset: CGFloat)?
  @State private var leaving: String?
  @State private var showsHint = false
  @AppStorage("chat-panes.hint-shown") private var hintShown = false
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  var body: some View {
    let deck = store.deck(chatID)
    let overview = store.isOverview(chatID)
    GeometryReader { proxy in
      let size = proxy.size
      let small = Self.overviewScale(size)
      let scale = Self.scale(magnification, overview: overview, small: small)
      let progress = min(1, max(0, (1 - scale) / (1 - small)))
      ScrollView(.horizontal) {
        HStack(spacing: 18 * progress) {
          ForEach(deck.pages) { page in
            card(page, size: size, scale: scale, progress: progress, overview: overview)
              .id(page.id)
          }
        }
        .scrollTargetLayout()
      }
      .scrollPosition($scroll, anchor: .center)
      .scrollTargetBehavior(.viewAligned(limitBehavior: overview ? .never : .always))
      .contentMargins(.horizontal, (size.width - size.width * scale) / 2, for: .scrollContent)
      .scrollIndicators(.hidden)
      .scrollClipDisabled()
      .background {
        // Pages run under the home indicator; the overview sits on a slightly lifted surface.
        ZStack {
          AppTheme.surface
          Color.primary.opacity(0.05 * progress)
        }
        .ignoresSafeArea()
      }
      .modifier(
        PinchToZoomOut(
          changed: { magnification = $0 },
          ended: { amount in endPinch(amount, overview: overview) })
      )
      .onAppear { scroll.scrollTo(id: deck.focus, anchor: .center) }
      // A swipe that settles on another page brings it to the front.
      .onScrollPhaseChange { _, phase in
        guard phase == .idle, magnification == nil, let id = scroll.viewID(type: String.self),
          id != store.deck(chatID).focus
        else { return }
        store.focus(chatID, id)
      }
      .onChange(of: deck.focus) { _, id in
        guard scroll.viewID(type: String.self) != id else { return }
        align(id, animated: true)
      }
      .onChange(of: deck.pageIDs) { _, _ in
        // A page opened in the same update as the focus moved: scroll once it is laid out.
        align(store.deck(chatID).focus, animated: true)
      }
      .onChange(of: overview) { _, _ in
        // Keep the page in front centred while the cards zoom.
        scroll.scrollTo(id: store.deck(chatID).focus, anchor: .center)
      }
      .onChange(of: deck.panes.count) { old, new in
        guard new > old, !hintShown, !Self.suppressesHint else { return }
        hintShown = true
        withAnimation(.easeOut(duration: 0.3)) { showsHint = true }
      }
    }
    .overlay(alignment: .trailing) { hint }
    #if DEBUG
      .task {
        // `--pane-hint` shows the one-time hint on every launch, for screenshots.
        if ProcessInfo.processInfo.arguments.contains("--pane-hint") {
          try? await Task.sleep(for: .milliseconds(600))
          withAnimation(.easeOut(duration: 0.3)) { showsHint = true }
        }
      }
    #endif
    .sensoryFeedback(.impact(weight: .medium), trigger: store.closedCount)
  }

  /// Screenshots of seeded panes skip the one-time hint unless they ask for it.
  private static var suppressesHint: Bool {
    #if DEBUG
      let arguments = ProcessInfo.processInfo.arguments
      return arguments.contains("--panes-preview") && !arguments.contains("--pane-hint")
    #else
      return false
    #endif
  }

  /// Cards are about 60% of the page on a phone and nearer half on an iPad or a Mac window.
  static func overviewScale(_ size: CGSize) -> CGFloat {
    guard size.width > 0 else { return 0.72 }
    return min(0.74, max(0.42, 460 / size.width))
  }

  // MARK: Cards

  private func card(
    _ page: ChatPanePage, size: CGSize, scale: CGFloat, progress: CGFloat, overview: Bool
  ) -> some View {
    let lifted: CGFloat = lift?.id == page.id ? lift?.offset ?? 0 : 0
    return face(page, size: size, scale: scale, progress: progress, overview: overview)
      .overlay {
        cardLabel(page, size: size, scale: scale, progress: progress, lifted: lifted)
      }
      .offset(y: -24 * progress + lifted)
      .opacity(leaving == page.id ? 0 : 1)
      .contentShape(Rectangle())
      .onTapGesture {
        guard overview, page != .selection else { return }
        bringForward(page.id)
      }
      .modifier(
        CardLift(
          enabled: overview && page.isClosable,
          changed: { offset in lift = (page.id, offset) },
          ended: { offset, velocity in endLift(page.id, offset, velocity, height: size.height) })
      )
      .overlay(alignment: .topLeading) {
        closeButton(page, size: size, scale: scale, progress: progress, overview: overview)
      }
      .modifier(
        CardAccessibility(
          // The picker's tiles stay their own buttons in the overview.
          overview: overview && page != .selection, closable: page.isClosable,
          // Full screen, only the page in front is read; the others are off screen.
          hidden: !overview && page.id != store.deck(chatID).focus,
          title: label(page)?.title ?? "Pane picker", id: page.id,
          open: { bringForward(page.id) }, close: { close(page.id) }))
  }

  /// The page itself, drawn at full size and scaled down to a card.
  private func face(
    _ page: ChatPanePage, size: CGSize, scale: CGFloat, progress: CGFloat, overview: Bool
  ) -> some View {
    let picker = page == .selection
    let shape = RoundedRectangle(cornerRadius: 22 * progress / scale, style: .continuous)
    let fill: AnyShapeStyle = picker ? AnyShapeStyle(.clear) : AnyShapeStyle(AppTheme.surface)
    let shadow = Color.black.opacity(picker ? 0 : 0.22 * progress)
    // As a card, the page draws a little inside its edges so its content gets a margin.
    let inset = picker ? 1 : 1 - 0.07 * progress
    return self.page(page)
      .frame(width: size.width, height: size.height)
      .scaleEffect(inset)
      .background(fill)
      .clipShape(shape)
      // In the overview a card is a picture of its page; only the picker's tiles stay live.
      .allowsHitTesting(!overview || picker)
      .scaleEffect(scale)
      .frame(width: size.width * scale, height: size.height)
      .shadow(color: shadow, radius: 18, y: 8)
  }

  @ViewBuilder private func cardLabel(
    _ page: ChatPanePage, size: CGSize, scale: CGFloat, progress: CGFloat, lifted: CGFloat
  ) -> some View {
    if let label = label(page), progress > 0.01 {
      let fade = progress * (1 - min(1, abs(lifted) / 200))
      HStack(spacing: 7) {
        AppIcon(name: label.symbol, size: 12, weight: .semibold).foregroundStyle(AppTheme.accent)
        Text(label.title).font(.system(size: 14, weight: .semibold)).lineLimit(1)
      }
      .padding(.horizontal, 12).frame(height: 30)
      .background(Color.primary.opacity(0.08), in: Capsule())
      .frame(maxWidth: size.width * scale)
      .offset(y: size.height * scale / 2 + 26)
      .opacity(fade)
      .allowsHitTesting(false)
    }
  }

  @ViewBuilder private func closeButton(
    _ page: ChatPanePage, size: CGSize, scale: CGFloat, progress: CGFloat, overview: Bool
  ) -> some View {
    #if os(macOS)
      if overview, page.isClosable {
        let top: CGFloat = size.height * (1 - scale) / 2 - 24 * progress - 8
        Button {
          close(page.id)
        } label: {
          Image(systemName: "xmark.circle.fill").font(.system(size: 20))
            .symbolRenderingMode(.hierarchical)
        }
        .buttonStyle(.plain)
        .offset(x: -8, y: top)
        .accessibilityLabel("Close")
      }
    #endif
  }

  /// Scrolls to a page after the current update, so pages added with it are laid out first.
  private func align(_ id: String, animated: Bool) {
    Task { @MainActor in
      await Task.yield()
      withAnimation(animated && !reduceMotion ? .snappy(duration: 0.35) : nil) {
        scroll.scrollTo(id: id, anchor: .center)
      }
    }
  }

  private func bringForward(_ id: String) {
    withAnimation(reduceMotion ? .easeOut(duration: 0.2) : .spring(duration: 0.42, bounce: 0.12)) {
      store.focus(chatID, id)
      scroll.scrollTo(id: id, anchor: .center)
      store.setOverview(chatID, false)
    }
  }

  private func endLift(_ id: String, _ offset: CGFloat, _ velocity: CGFloat, height: CGFloat) {
    let flung = offset < -height * 0.22 || (velocity < -700 && offset < -30)
    guard flung else {
      withAnimation(.spring(duration: 0.35, bounce: 0.25)) { lift = nil }
      return
    }
    let distance = height + 120
    withAnimation(.easeOut(duration: 0.22)) { lift = (id, -distance) }
    Task { @MainActor in
      try? await Task.sleep(for: .milliseconds(200))
      leaving = id
      withAnimation(.snappy(duration: 0.32)) {
        store.close(chatID, id, swiped: true)
      }
      lift = nil
      leaving = nil
    }
  }

  private func close(_ id: String) {
    withAnimation(.snappy(duration: 0.32)) { store.close(chatID, id, swiped: true) }
  }

  // MARK: Pinch

  /// The cards' scale: 1 is full screen and `small` the overview, following a pinch live.
  private static func scale(_ magnification: CGFloat?, overview: Bool, small: CGFloat) -> CGFloat {
    guard let magnification else { return overview ? small : 1 }
    let live = overview ? small * magnification : 1 - (1 - magnification) * 1.15
    return min(1, max(small, live))
  }

  /// Pinching in past a little zooms out; pinching out over the cards zooms back in. A
  /// cancelled pinch (`nil`) springs back.
  private func endPinch(_ amount: CGFloat?, overview: Bool) {
    let animation: Animation =
      reduceMotion ? .easeOut(duration: 0.2) : .spring(duration: 0.4, bounce: 0.14)
    withAnimation(animation) {
      magnification = nil
      if let amount { store.setOverview(chatID, overview ? amount < 1.12 : amount < 0.9) }
    }
  }

  // MARK: Hint

  /// An edge cue: two right-aligned lines beside a thin handle at the screen's edge, over a
  /// soft fade instead of a box. The handle nudges inward, pointing the way to swipe.
  @ViewBuilder private var hint: some View {
    if showsHint {
      PaneEdgeHint()
        .transition(.move(edge: .trailing).combined(with: .opacity))
        .task {
          try? await Task.sleep(for: .seconds(4))
          withAnimation(.easeIn(duration: 0.3)) { showsHint = false }
        }
        .onTapGesture { withAnimation { showsHint = false } }
    }
  }
}

private struct PaneEdgeHint: View {
  @Environment(\.colorScheme) private var colorScheme
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var nudged = false
  var body: some View {
    let dark = colorScheme == .dark
    HStack(spacing: 14) {
      Text("Swipe or pinch\nto see all panes")
        .font(.system(size: 17))
        .multilineTextAlignment(.trailing)
        .foregroundStyle(.primary)
        .offset(x: nudged ? -6 : 0)
      Capsule().fill(.primary).frame(width: 4, height: 46)
        .offset(x: nudged ? -6 : 0)
    }
    .padding(.trailing, 10).padding(.vertical, 40).padding(.leading, 70)
    .background {
      // A soft, blurred patch of the page colour keeps the words readable over the transcript
      // without drawing a card.
      Ellipse()
        .fill(AppTheme.surface)
        .padding(.trailing, -80)
        .blur(radius: 24)
        .opacity(dark ? 0.97 : 0.95)
        .allowsHitTesting(false)
    }
    .onAppear {
      guard !reduceMotion else { return }
      withAnimation(.easeInOut(duration: 0.7).repeatForever(autoreverses: true).delay(0.3)) {
        nudged = true
      }
    }
    .accessibilityElement(children: .ignore)
    .accessibilityLabel("Swipe or pinch to see all panes")
  }
}

/// Two-finger pinch over the pages. On iPhone and iPad it is a UIKit pinch that recognises
/// alongside the scroll views inside, so the transcript and lists keep scrolling.
private struct PinchToZoomOut: ViewModifier {
  let changed: (CGFloat) -> Void
  let ended: (CGFloat?) -> Void

  func body(content: Content) -> some View {
    #if os(iOS)
      content.gesture(PagePinch(changed: changed, ended: ended))
    #else
      content.simultaneousGesture(
        MagnifyGesture(minimumScaleDelta: 0.03)
          .onChanged { changed($0.magnification) }
          .onEnded { ended($0.magnification) })
    #endif
  }
}

#if os(iOS)
  private struct PagePinch: UIGestureRecognizerRepresentable {
    let changed: (CGFloat) -> Void
    let ended: (CGFloat?) -> Void

    func makeUIGestureRecognizer(context: Context) -> UIPinchGestureRecognizer {
      let pinch = UIPinchGestureRecognizer()
      pinch.delegate = context.coordinator
      return pinch
    }

    func makeCoordinator(converter: CoordinateSpaceConverter) -> Coordinator { Coordinator() }

    func handleUIGestureRecognizerAction(_ pinch: UIPinchGestureRecognizer, context: Context) {
      switch pinch.state {
      case .began, .changed: changed(pinch.scale)
      case .ended: ended(pinch.scale)
      case .cancelled, .failed: ended(nil)
      default: break
      }
    }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
      func gestureRecognizer(
        _ gesture: UIGestureRecognizer,
        shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer
      ) -> Bool { true }
    }
  }
#endif

/// In the overview each card is one button: open it, or close it with the Close action.
private struct CardAccessibility: ViewModifier {
  let overview: Bool
  let closable: Bool
  let hidden: Bool
  let title: String
  let id: String
  let open: () -> Void
  let close: () -> Void
  func body(content: Content) -> some View {
    content
      .accessibilityHidden(hidden)
      .accessibilityElement(children: overview ? .ignore : .contain)
      .accessibilityLabel(overview ? title : "")
      .accessibilityAddTraits(overview ? .isButton : [])
      .accessibilityAction { if overview { open() } }
      .accessibilityActions {
        if overview && closable { Button("Close", action: close) }
      }
      .accessibilityIdentifier("pane-card-" + id)
  }
}

/// Swipe a card up to close it. On iPhone and iPad the drag only starts when it is mostly
/// vertical, so sideways swipes keep scrolling the cards.
private struct CardLift: ViewModifier {
  let enabled: Bool
  let changed: (CGFloat) -> Void
  let ended: (CGFloat, CGFloat) -> Void

  func body(content: Content) -> some View {
    // Always attached, so turning it on and off never rebuilds the page underneath.
    #if os(iOS)
      content.gesture(VerticalPan(enabled: enabled, changed: changed, ended: ended))
    #else
      content
    #endif
  }
}

#if os(iOS)
  /// A pan that begins only for a mostly vertical drag and makes the scroll view wait for it,
  /// so lifting a card never also scrolls the row of cards.
  private struct VerticalPan: UIGestureRecognizerRepresentable {
    let enabled: Bool
    let changed: (CGFloat) -> Void
    let ended: (CGFloat, CGFloat) -> Void

    func makeUIGestureRecognizer(context: Context) -> UIPanGestureRecognizer {
      let pan = UIPanGestureRecognizer()
      pan.delegate = context.coordinator
      pan.maximumNumberOfTouches = 1
      pan.isEnabled = enabled
      return pan
    }

    func updateUIGestureRecognizer(_ pan: UIPanGestureRecognizer, context: Context) {
      pan.isEnabled = enabled
    }

    func makeCoordinator(converter: CoordinateSpaceConverter) -> Coordinator { Coordinator() }

    func handleUIGestureRecognizerAction(_ pan: UIPanGestureRecognizer, context: Context) {
      let translation = pan.translation(in: pan.view).y
      // Pulling down only stretches a little.
      let offset = translation < 0 ? translation : translation * 0.18
      switch pan.state {
      case .began, .changed: changed(offset)
      case .ended: ended(offset, pan.velocity(in: pan.view).y)
      case .cancelled, .failed: ended(0, 0)
      default: break
      }
    }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
      func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
        guard let pan = gesture as? UIPanGestureRecognizer else { return false }
        let velocity = pan.velocity(in: pan.view)
        return abs(velocity.y) > abs(velocity.x) * 1.2
      }
      func gestureRecognizer(
        _ gesture: UIGestureRecognizer,
        shouldBeRequiredToFailBy other: UIGestureRecognizer
      ) -> Bool {
        other.view is UIScrollView
      }
    }
  }
#endif

/// Dots under a chat's title: how many pages it has and which is in front.
struct ChatPageDots: View {
  let count: Int
  let index: Int
  /// Tapping the dots zooms out to every pane.
  var action: (() -> Void)?
  /// Solid ink: the navigation bar's vibrancy would otherwise wash the current dot out. A
  /// system colour, so it also resolves while SwiftUI renders off the main thread.
  #if os(macOS)
    private static let ink = Color(nsColor: .labelColor)
  #else
    private static let ink = Color(uiColor: .label)
  #endif
  var body: some View {
    if let action {
      Button(action: action) {
        dots.padding(.horizontal, 10).padding(.vertical, 6).contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .accessibilityLabel("Page \(index + 1) of \(count)")
      .accessibilityHint("Shows all panes")
      .accessibilityIdentifier("pane-dots")
    } else {
      dots
    }
  }
  private var dots: some View {
    HStack(spacing: 5) {
      ForEach(0..<min(count, 9), id: \.self) { dot in
        let current = dot == min(index, 8)
        Circle()
          .fill(current ? Self.ink : Self.ink.opacity(0.25))
          .frame(width: current ? 6 : 5, height: current ? 6 : 5)
      }
    }
    .animation(.snappy(duration: 0.25), value: index)
    .animation(.snappy(duration: 0.25), value: count)
    .accessibilityElement()
    .accessibilityLabel("Page \(index + 1) of \(count)")
    .accessibilityIdentifier("pane-dots")
  }
}
