import SwiftUI
import ZuseCore

extension EnvironmentValues {
  /// Room the iPad's floating tab bar takes at the bottom of a list.
  @Entry var tabletBarClearance: CGFloat = 0
  /// The iPad's full-width lists: rows take roomier type and keep long lines to a readable
  /// measure.
  @Entry var tabletLists = false
}

/// Lets a tab's list scroll its last rows, and anything pinned under it, clear of the iPad's
/// floating tab bar. Applied to each list rather than the shell: safe area set outside a
/// navigation stack doesn't reach the list inside it.
private struct TabletBarClearance: ViewModifier {
  @Environment(\.tabletBarClearance) private var clearance
  func body(content: Content) -> some View {
    content.safeAreaPadding(.bottom, clearance)
  }
}

extension View {
  func clearsTabletBar() -> some View { modifier(TabletBarClearance()) }
}

/// The longest a line of text runs in an iPad list before it truncates, so a preview on a
/// 13-inch screen in landscape still reads as one glance.
enum TabletMeasure {
  static let line: CGFloat = 920
}

#if os(iOS)
  /// The iPad layout: every tab is one full-width list, with a compact floating tab bar centred
  /// along the bottom in either orientation. Opening a row pushes its page over the whole
  /// window, and the page covers the bar, as pages cover the iPhone tab bar.
  struct TabletShell<Content: View>: View {
    @Binding var tab: AppTab
    let needsYou: Int
    let account: Account
    /// A page is pushed over the tab's list.
    let showsPage: Bool
    @ViewBuilder let content: Content
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// The on-screen keyboard is up: the bar steps aside rather than riding above it.
    @State private var typing = false

    private var barShown: Bool { !showsPage && !typing }

    var body: some View {
      content
        .environment(\.tabletBarClearance, typing ? 0 : TabletTabBar.clearance)
        .environment(\.tabletLists, true)
        .overlay(alignment: .bottom) {
          if barShown {
            TabletTabBar(selection: $tab, needsYou: needsYou, account: account)
              .padding(.bottom, 10)
              .transition(
                reduceMotion ? .opacity : .opacity.combined(with: .move(edge: .bottom)))
          }
        }
        .animation(reduceMotion ? nil : .snappy(duration: 0.28), value: barShown)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(AppTheme.surface.ignoresSafeArea())
        .onReceive(
          NotificationCenter.default.publisher(for: UIResponder.keyboardWillShowNotification)
        ) { note in
          // A hardware keyboard shows only its slim shortcut bar; that leaves the bar in place.
          let frame = note.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? CGRect
          typing = (frame?.height ?? 0) > 120
        }
        .onReceive(
          NotificationCenter.default.publisher(for: UIResponder.keyboardWillHideNotification)
        ) { _ in
          typing = false
        }
    }
  }

  /// The iPad tab bar: icon-only, on one capsule of glass. The selected tab takes the accent
  /// colour on a soft pill; More is your account picture once signed in.
  struct TabletTabBar: View {
    @Binding var selection: AppTab
    let needsYou: Int
    let account: Account
    @Namespace private var pill
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// Room a list leaves under its last row so it can scroll clear of the bar.
    static let clearance: CGFloat = 72

    var body: some View {
      HStack(spacing: 2) {
        ForEach(AppTab.allCases, id: \.self) { tab in item(tab) }
      }
      .padding(4)
      .modifier(TabletBarGlass())
      .accessibilityElement(children: .contain)
      .accessibilityLabel("Tabs")
    }

    private func item(_ tab: AppTab) -> some View {
      let selected = selection == tab
      return Button {
        guard selection != tab else { return }
        withAnimation(reduceMotion ? nil : .snappy(duration: 0.25)) { selection = tab }
      } label: {
        icon(tab, selected: selected)
          .foregroundStyle(selected ? AnyShapeStyle(AppTheme.accent) : AnyShapeStyle(.primary))
          .frame(width: 52, height: 42)
          .background {
            if selected {
              Capsule().fill(Color.primary.opacity(0.09))
                .matchedGeometryEffect(id: "pill", in: pill)
            }
          }
          .overlay(alignment: .topTrailing) {
            if tab == .zappers && needsYou > 0 {
              Text(needsYou > 99 ? "99+" : "\(needsYou)")
                .font(.system(size: 11, weight: .semibold)).monospacedDigit()
                .foregroundStyle(.white)
                .padding(.horizontal, 5).frame(minWidth: 18, minHeight: 18)
                .background(Color.orange, in: Capsule())
                .offset(x: -2, y: -1)
                .accessibilityHidden(true)
            }
          }
          .contentShape(Capsule())
      }
      .buttonStyle(.plain)
      .hoverEffect(.highlight)
      .accessibilityLabel(Self.title(tab))
      .accessibilityValue(tab == .zappers && needsYou > 0 ? "\(needsYou) waiting for you" : "")
      .accessibilityAddTraits(selected ? .isSelected : [])
      .accessibilityIdentifier("tab-" + tab.rawValue)
      .keyboardShortcut(Self.shortcut(tab), modifiers: .command)
    }

    @ViewBuilder private func icon(_ tab: AppTab, selected: Bool) -> some View {
      switch tab {
      case .zappers: AppIcon(name: AppIcon.zapper, size: 19)
      case .chats:
        Image(systemName: "bubble.left.and.bubble.right").font(.system(size: 17))
      case .prs: AppIcon(name: AppIcon.pullRequest, size: 18)
      case .more:
        if account.signedIn {
          AccountAvatar(account: account, size: 26)
            .overlay {
              if selected {
                Circle().strokeBorder(AppTheme.accent, lineWidth: 2).padding(-3)
              }
            }
        } else {
          Image(systemName: "ellipsis.circle").font(.system(size: 19))
        }
      case .search: Image(systemName: "magnifyingglass").font(.system(size: 18, weight: .medium))
      }
    }

    static func title(_ tab: AppTab) -> String {
      switch tab {
      case .zappers: return "Zappers"
      case .chats: return "Chats"
      case .prs: return "Pull requests"
      case .more: return "More"
      case .search: return "Search"
      }
    }

    private static func shortcut(_ tab: AppTab) -> KeyEquivalent {
      switch tab {
      case .zappers: return "1"
      case .chats: return "2"
      case .prs: return "3"
      case .more: return "4"
      case .search: return "f"
      }
    }
  }

  /// Liquid Glass on iOS 26, a thin material with a hairline before it.
  private struct TabletBarGlass: ViewModifier {
    func body(content: Content) -> some View {
      if #available(iOS 26, *) {
        GlassEffectContainer {
          content.glassEffect(.regular.interactive(), in: Capsule())
        }
      } else {
        content
          .background(.ultraThinMaterial, in: Capsule())
          .overlay { Capsule().strokeBorder(Color.primary.opacity(0.08), lineWidth: 0.5) }
          .shadow(color: .black.opacity(0.12), radius: 16, y: 6)
      }
    }
  }
#endif

/// A list's two top-trailing buttons. On iPad each stands on its own round of glass; elsewhere
/// they share one capsule, as the system groups them.
struct PrimaryActions<Leading: View, Trailing: View>: ViewModifier {
  let separate: Bool
  let leading: Leading
  let trailing: Trailing

  init(
    separate: Bool, @ViewBuilder leading: () -> Leading, @ViewBuilder trailing: () -> Trailing
  ) {
    self.separate = separate
    self.leading = leading()
    self.trailing = trailing()
  }

  func body(content: Content) -> some View {
    #if os(iOS)
      if #available(iOS 26, *), separate {
        content.toolbar {
          ToolbarItem(placement: .primaryAction) { leading }
          ToolbarSpacer(.fixed, placement: .primaryAction)
          ToolbarItem(placement: .primaryAction) { trailing }
        }
      } else {
        grouped(content)
      }
    #else
      grouped(content)
    #endif
  }

  private func grouped(_ content: Content) -> some View {
    content.toolbar {
      ToolbarItemGroup(placement: .primaryAction) {
        leading
        trailing
      }
    }
  }
}

/// "Ask anything" floating at the bottom of an iPad page that has no composer of its own:
/// opens a new chat ready to write.
struct TabletAskPill: View {
  let ask: () -> Void
  var body: some View {
    Button(action: ask) {
      HStack(spacing: 10) {
        Image(systemName: "plus").font(.system(size: 17, weight: .medium))
          .foregroundStyle(.primary).frame(width: 34, height: 34)
          .background(Color.primary.opacity(0.07), in: Circle())
        Text("Ask anything").font(.system(size: 17)).foregroundStyle(.secondary)
        Spacer(minLength: 0)
      }
      .padding(.leading, 7).padding(.trailing, 14)
      .frame(minHeight: 48).contentShape(Capsule())
    }
    .buttonStyle(.plain)
    .composerGlass(radius: 24)
    .padding(.horizontal, 24).padding(.bottom, 6).padding(.top, 4)
    .accessibilityLabel("New chat")
    .accessibilityIdentifier("ask-anything")
  }
}
