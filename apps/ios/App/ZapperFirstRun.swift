import SwiftUI
import ZuseCore

/// A ready-made goal for a first Zapper: a name, what it looks after and a face.
struct ZapperStarter: Identifiable, Hashable {
  let name: String
  let purpose: String
  let avatar: ZapperAvatar
  var id: String { name }

  static let all: [ZapperStarter] = [
    ZapperStarter(
      name: "Release lead",
      purpose:
        "Keep main green, land pull requests that are ready and ship a release when it’s time.",
      avatar: ZapperAvatar(mascot: "bolt", color: "green")),
    ZapperStarter(
      name: "Bug triager",
      purpose: "Sort new issues, reproduce the clear ones and fix the small ones.",
      avatar: ZapperAvatar(mascot: "ace", color: "coral")),
    ZapperStarter(
      name: "Docs gardener",
      purpose: "Keep the docs in step with the code and fix whatever drifts.",
      avatar: ZapperAvatar(mascot: "squire", color: "violet")),
  ]
}

/// Three faces leaning together: the picture for having no Zappers yet.
struct ZapperTrio: View {
  var size: CGFloat = 64
  var body: some View {
    let avatars = ZapperStarter.all.map(\.avatar)
    HStack(spacing: -size * 0.22) {
      ForEach(Array(avatars.enumerated()), id: \.offset) { index, avatar in
        let middle = index == 1
        ZapperFace(avatar: avatar, size: middle ? size : size * 0.82)
          .rotationEffect(.degrees(index == 0 ? -8 : index == 2 ? 8 : 0))
          .offset(y: middle ? -size * 0.08 : 0)
          .zIndex(middle ? 1 : 0)
      }
    }
    .accessibilityHidden(true)
  }
}

/// A mascot on a soft tile of its colour, for faces that aren't a saved Zapper yet.
struct ZapperFace: View {
  let avatar: ZapperAvatar
  var size: CGFloat = 40
  var body: some View {
    let (_, color) = ZapperMascots.resolve(avatar)
    let shape = RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
    ZapperMascotView(avatar: avatar, size: size * 0.72)
      .frame(width: size, height: size)
      .background {
        shape.fill(AppTheme.card)
        shape.fill(Color(hex: color.body).opacity(0.18))
      }
      .overlay { shape.strokeBorder(Color.primary.opacity(0.06), lineWidth: 0.5) }
  }
}

/// The Zappers tab with nothing in it yet: what a Zapper is, one clear way to make one, and
/// three ideas to start from.
struct ZapperFirstRun: View {
  let create: (ZapperStarter?) -> Void
  var body: some View {
    VStack(spacing: 0) {
      ZapperTrio(size: 72).padding(.bottom, 22)
      Text("Meet your first Zapper").font(.title2.weight(.semibold))
        .multilineTextAlignment(.center)
      Text(
        "A Zapper looks after one goal for you. It plans the work, starts threads and asks when it needs you."
      )
      .font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
      .frame(maxWidth: 320).padding(.top, 8)
      Button {
        create(nil)
      } label: {
        Label("Create a Zapper", systemImage: "plus").font(.body.weight(.semibold))
          .frame(maxWidth: 280).frame(minHeight: 30)
      }
      .buttonStyle(.borderedProminent).tint(AppTheme.accent).controlSize(.large)
      .buttonBorderShape(.capsule)
      .padding(.top, 22)
      .accessibilityIdentifier("create-first-zapper")
      VStack(alignment: .leading, spacing: 8) {
        Text("Or start from an idea").font(.footnote.weight(.medium)).foregroundStyle(.secondary)
          .padding(.leading, 4)
        VStack(spacing: 0) {
          ForEach(Array(ZapperStarter.all.enumerated()), id: \.element.id) { index, starter in
            if index > 0 {
              Rectangle().fill(AppTheme.hairline).frame(height: 1).padding(.leading, 58)
            }
            Button {
              create(starter)
            } label: {
              StarterRow(starter: starter)
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("zapper-starter-\(index)")
          }
        }
        .background(AppTheme.card, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay {
          RoundedRectangle(cornerRadius: 16, style: .continuous)
            .strokeBorder(AppTheme.hairline, lineWidth: 1)
        }
      }
      .frame(maxWidth: 420)
      .padding(.top, 36)
    }
    .frame(maxWidth: .infinity)
    .padding(.horizontal, 20)
  }
}

private struct StarterRow: View {
  let starter: ZapperStarter
  var body: some View {
    HStack(spacing: 12) {
      ZapperFace(avatar: starter.avatar, size: 34)
      VStack(alignment: .leading, spacing: 1) {
        Text(starter.name).font(.system(size: 16, weight: .medium)).foregroundStyle(.primary)
        Text(starter.purpose).font(.system(size: 13)).foregroundStyle(.secondary).lineLimit(1)
      }
      Spacer(minLength: 6)
      Image(systemName: "chevron.right").font(.system(size: 12, weight: .semibold))
        .foregroundStyle(.tertiary)
    }
    .padding(.horizontal, 12).frame(minHeight: 60).contentShape(Rectangle())
    .accessibilityElement(children: .combine)
    .accessibilityHint("Creates a Zapper from this idea")
  }
}

/// The sidebar's Zappers section with none yet: a small invitation in place of the list.
struct ZapperSidebarInvite: View {
  let create: () -> Void
  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      ZapperTrio(size: 34)
      VStack(alignment: .leading, spacing: 2) {
        Text("Create your first Zapper").font(.subheadline.weight(.semibold))
        Text("It looks after one goal and asks when it needs you.")
          .font(.footnote).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
      }
      Button(action: create) {
        Label("New Zapper", systemImage: "plus").font(.subheadline.weight(.semibold))
          .frame(maxWidth: .infinity)
      }
      .buttonStyle(.borderedProminent).tint(AppTheme.accent).controlSize(.regular)
      .buttonBorderShape(.capsule)
      .accessibilityIdentifier("sidebar-create-first-zapper")
    }
    .padding(14)
    .background(AppTheme.card, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
    .overlay {
      RoundedRectangle(cornerRadius: 16, style: .continuous)
        .strokeBorder(AppTheme.hairline, lineWidth: 1)
    }
  }
}

/// Quiet stand-ins for rows that are still loading: the shape of the list, gently breathing,
/// instead of a spinner.
struct LoadingRows: View {
  var count = 4
  var tile: CGFloat = 40
  var circle = false
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var dim = false
  private let titles: [CGFloat] = [150, 118, 172, 136]
  private let details: [CGFloat] = [232, 196, 248, 184]
  var body: some View {
    VStack(spacing: 20) {
      ForEach(0..<count, id: \.self) { index in
        HStack(spacing: 12) {
          if circle {
            Circle().frame(width: tile, height: tile)
          } else {
            RoundedRectangle(cornerRadius: tile * 0.27, style: .continuous)
              .frame(width: tile, height: tile)
          }
          VStack(alignment: .leading, spacing: 9) {
            Capsule().frame(width: titles[index % 4], height: 11)
            Capsule().frame(width: details[index % 4], height: 9).opacity(0.7)
          }
          Spacer(minLength: 0)
        }
      }
    }
    .foregroundStyle(Color.primary.opacity(0.07))
    .opacity(dim ? 0.55 : 1)
    .onAppear {
      guard !reduceMotion else { return }
      withAnimation(.easeInOut(duration: 1.1).repeatForever(autoreverses: true)) { dim = true }
    }
    .padding(.vertical, 8)
    .accessibilityElement()
    .accessibilityLabel("Loading")
  }
}

/// A conversation with nothing in it yet: loading, a Zapper waiting for its first message, or
/// a fresh chat.
struct ChatEmptyState: View {
  let zapper: Zapper?
  let loading: Bool
  var body: some View {
    Group {
      if loading {
        VStack(spacing: 10) {
          ProgressView()
          Text("Loading chat").font(.footnote).foregroundStyle(.secondary)
        }
      } else if let zapper {
        VStack(spacing: 0) {
          ZapperTile(zapper: zapper, size: 76).padding(.bottom, 18)
          Text("\(zapper.config.name) is ready").font(.title3.weight(.semibold))
          if !zapper.config.purpose.isEmpty {
            Text(zapper.config.purpose).font(.subheadline).foregroundStyle(.secondary)
              .lineLimit(3).padding(.top, 6)
          }
          Text("Tell it what to look at first.").font(.footnote).foregroundStyle(.tertiary)
            .padding(.top, 14)
        }
        .multilineTextAlignment(.center).frame(maxWidth: 320)
      } else {
        VStack(spacing: 8) {
          Image(systemName: "bubble.left.and.text.bubble.right").font(
            .system(size: 30, weight: .light)
          )
          .foregroundStyle(.tertiary).padding(.bottom, 4)
          Text("Nothing here yet").font(.headline)
          Text("Ask anything to get started.").font(.subheadline).foregroundStyle(.secondary)
        }
      }
    }
    .padding(.horizontal, 32)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .accessibilityElement(children: .combine)
  }
}

/// A form row's title with a quiet glyph, so the rows read as settings rather than buttons.
struct FormRowLabel: View {
  let title: String
  let symbol: String
  var body: some View {
    Label {
      Text(title).foregroundStyle(.primary)
    } icon: {
      Image(systemName: symbol).foregroundStyle(.secondary)
    }
  }
}

/// Every face and colour a Zapper can wear, as two rows of small swatches.
struct ZapperLookPicker: View {
  @Binding var avatar: ZapperAvatar
  var body: some View {
    let (mascot, color) = ZapperMascots.resolve(avatar)
    VStack(spacing: 14) {
      ScrollView(.horizontal) {
        HStack(spacing: 8) {
          ForEach(ZapperMascots.mascots) { option in
            let selected = option.id == mascot.id
            Button {
              withAnimation(.snappy) { avatar = ZapperAvatar(mascot: option.id, color: color.id) }
            } label: {
              ZapperMascotView(
                avatar: ZapperAvatar(mascot: option.id, color: color.id), size: 30
              )
              .frame(width: 46, height: 46)
              .background(
                Color(hex: color.body).opacity(selected ? 0.22 : 0.08),
                in: RoundedRectangle(cornerRadius: 13, style: .continuous)
              )
              .overlay {
                RoundedRectangle(cornerRadius: 13, style: .continuous)
                  .strokeBorder(selected ? AppTheme.accent : .clear, lineWidth: 2)
              }
            }
            .buttonStyle(.plain)
            .accessibilityLabel(option.name)
            .accessibilityAddTraits(selected ? .isSelected : [])
          }
        }
        .padding(.horizontal, 16).padding(.vertical, 2)
      }
      .scrollIndicators(.hidden)
      HStack(spacing: 12) {
        ForEach(ZapperMascots.colors) { option in
          let selected = option.id == color.id
          Button {
            withAnimation(.snappy) { avatar = ZapperAvatar(mascot: mascot.id, color: option.id) }
          } label: {
            Circle().fill(Color(hex: option.body)).frame(width: 24, height: 24)
              .padding(3)
              .overlay {
                Circle().strokeBorder(selected ? Color.primary.opacity(0.7) : .clear, lineWidth: 2)
              }
              .frame(width: 34, height: 34).contentShape(Circle())
          }
          .buttonStyle(.plain)
          .accessibilityLabel(option.id.capitalized)
          .accessibilityAddTraits(selected ? .isSelected : [])
        }
      }
    }
  }
}
