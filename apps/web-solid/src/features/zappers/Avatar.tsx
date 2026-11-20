import { createMemo, For } from "solid-js";
import type { Zapper, ZapperAvatar } from "@/core/zapper-types";
import { mascots, colors } from "./mascots";

export function defaultAvatar(seed: string): ZapperAvatar {
  let hash = 0x811c9dc5;
  for (const byte of new TextEncoder().encode(seed))
    hash = Math.imul(hash ^ byte, 0x01000193) >>> 0;
  return {
    mascot: mascots[hash % mascots.length]!.id,
    color: colors[(hash >>> 8) % colors.length]!.id,
  };
}
export const avatarOf = (zot: Zapper) =>
  zot.config.avatar ?? defaultAvatar(zot.id);
export function Avatar(props: { avatar: ZapperAvatar; size?: number }) {
  const pixels = createMemo(() => {
    const sprite = (
      mascots.find((m) => m.id === props.avatar.mascot) ?? mascots[0]!
    ).sprite;
    const color = colors.find((c) => c.id === props.avatar.color) ?? colors[0]!;
    const palette: Record<string, string> = {
      b: color.body,
      s: color.shade,
      k: "#1d1418",
      w: "#ffffff",
      c: "#b9c3d1",
      g: "#7fe9ff",
      t: "#ff7aa8",
      p: "#f2c199",
    };
    return sprite.flatMap((row, y) =>
      [...row].flatMap((pixel, x) =>
        palette[pixel] ? [{ x, y, fill: palette[pixel]! }] : [],
      ),
    );
  });
  return (
    <svg
      class="zp-avatar"
      viewBox="0 0 16 16"
      width={props.size ?? 20}
      height={props.size ?? 20}
      aria-hidden="true"
      shape-rendering="crispEdges"
    >
      <For each={pixels()}>
        {(pixel) => (
          <rect
            x={pixel.x}
            y={pixel.y}
            width="1"
            height="1"
            fill={pixel.fill}
          />
        )}
      </For>
    </svg>
  );
}
