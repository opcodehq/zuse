import SwiftUI
import ZuseCore

/// How the iPad layout sits in its window: the tab bar's edge, and whether pages open beside
/// their list or are pushed over it.
struct TabletLayout: Equatable {
  /// Wider than tall: the tab bar runs down the leading edge instead of along the bottom.
  var landscape = false
  /// Room for a list and its page side by side.
  var split = false

  init(landscape: Bool = false, split: Bool = false) {
    self.landscape = landscape
    self.split = split
  }

  init(size: CGSize) {
    landscape = size.width > size.height
    // A landscape window too narrow for a 380-point list beside a readable page pushes
    // pages instead, as portrait does.
    split = landscape && size.width >= 900
  }

  /// The list column beside an open page.
  static let listWidth: CGFloat = 380
}

/// A split view's list column: a fixed width that always shows on iPad, the given range
/// elsewhere.
struct TabletListColumn: ViewModifier {
  let enabled: Bool
  let min: CGFloat
  let ideal: CGFloat
  let max: CGFloat
  func body(content: Content) -> some View {
    if enabled {
      content.navigationSplitViewColumnWidth(TabletLayout.listWidth)
        .toolbar(removing: .sidebarToggle)
    } else {
      content.navigationSplitViewColumnWidth(min: min, ideal: ideal, max: max)
    }
  }
}

extension EnvironmentValues {
  /// Room the iPad's floating tab bar takes at the bottom of a list.
  @Entry var tabletBarClearance: CGFloat = 0
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

#if os(iOS)
  /// The iPad layout: the tab's content filling the window, with a compact floating tab bar
  /// along the bottom in portrait and down the leading edge in landscape. Pages pushed in
  /// portrait cover the bar, as they cover the iPhone tab bar.
  struct TabletShell<Content: View>: View {
    @Binding var tab: AppTab
    @Binding var layout: TabletLayout
    let needsYou: Int
    let account: Account
    /// A page is pushed over the tab's list.
    let showsPage: Bool
    @ViewBuilder let content: Content
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// The window has been measured once.
    @State private var measured = false

    private var barShown: Bool { layout.landscape || !showsPage }

    var body: some View {
      ZStack {
        // The tabs wait for the window's first measurement, so they open in the right layout
        // rather than switching from portrait's on the first frame.
        if measured { shell }
      }
      .frame(maxWidth: .infinity, maxHeight: .infinity)
      .background(AppTheme.surface.ignoresSafeArea())
      .onGeometryChange(for: TabletLayout.self) { proxy in
        TabletLayout(size: proxy.size)
      } action: { value in
        layout = value
        measured = true
      }
    }

    private var shell: some View {
      content
        // Lists scroll under the floating bar in portrait (see `clearsTabletBar`) and stop short
        // of it in landscape.
        .environment(\.tabletBarClearance, layout.landscape ? 0 : TabletTabBar.clearance)
        .padding(.leading, layout.landscape ? TabletTabBar.railClearance : 0)
        .overlay(alignment: layout.landscape ? .leading : .bottom) {
          if barShown {
            TabletTabBar(
              selection: $tab, vertical: layout.landscape, needsYou: needsYou, account: account
            )
            .padding(layout.landscape ? .leading : .bottom, layout.landscape ? 14 : 10)
            .transition(
              reduceMotion
                ? .opacity
                : .opacity.combined(with: .move(edge: layout.landscape ? .leading : .bottom))
            )
          }
        }
        .animation(reduceMotion ? nil : .snappy(duration: 0.28), value: barShown)
    }
  }

  /// The iPad tab bar: icon-only, on one capsule of glass. The selected tab takes the accent
  /// colour on a soft pill; More is your account picture once signed in.
  struct TabletTabBar: View {
    @Binding var selection: AppTab
    let vertical: Bool
    let needsYou: Int
    let account: Account
    @Namespace private var pill
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// Room a portrait list leaves under its last row so it can scroll clear of the bar.
    static let clearance: CGFloat = 76
    /// Room landscape content leaves on the leading edge for the bar.
    static let railClearance: CGFloat = 84
    private static let item: CGFloat = 50

    var body: some View {
      let stack =
        vertical
        ? AnyLayout(VStackLayout(spacing: 2)) : AnyLayout(HStackLayout(spacing: 2))
      stack {
        ForEach(AppTab.allCases, id: \.self) { tab in item(tab) }
      }
      .padding(5)
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
          .frame(width: Self.item + (vertical ? 0 : 8), height: Self.item - (vertical ? 0 : 6))
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
                .offset(x: vertical ? -4 : 0, y: 2)
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
        Image(systemName: "bubble.left.and.bubble.right").font(.system(size: 18))
      case .prs: AppIcon(name: AppIcon.pullRequest, size: 18)
      case .more:
        if account.signedIn {
          AccountAvatar(account: account, size: 28)
            .overlay {
              if selected {
                Circle().strokeBorder(AppTheme.accent, lineWidth: 2).padding(-3)
              }
            }
        } else {
          Image(systemName: "ellipsis.circle").font(.system(size: 20))
        }
      case .search: Image(systemName: "magnifyingglass").font(.system(size: 19, weight: .medium))
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
