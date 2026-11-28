import { createEffect, createMemo, createSignal, onSettled, Show } from "solid-js";
import type { Zapper, ZapperAvatar } from "@/core/zapper-types";
import { state } from "@/core/store";
import { deviceOnline } from "@/core/util";
import { indicatorFor } from "@/features/sidebar/model";
import { attach } from "./mascot-clock";
import { gridFor, hash, type Mood } from "./mascot-motion";
import { mascots, colors, EYE } from "./mascots";
import { moodFor } from "./mood";

export function defaultAvatar(seed: string): ZapperAvatar {
  const value = hash(seed);
  return {
    mascot: mascots[value % mascots.length]!.id,
    color: colors[(value >>> 8) % colors.length]!.id,
  };
}
export const avatarOf = (zapper: Zapper) =>
  zapper.config.avatar ?? defaultAvatar(zapper.id);

/** The mood a Zapper shows at `at`: chat statuses are staleness-gated exactly like the sidebar's dots. */
export function moodOf(zapper: Zapper, at: number): Mood {
  const status = (id: string) => {
    const chat = state.chats[id];
    return chat ? indicatorFor(chat, at) : undefined;
  };
  return moodFor(
    zapper,
    deviceOnline(zapper.config.homeDeviceId, at),
    status(zapper.coordinatorChatId),
    zapper.workers.flatMap(worker => status(worker.chatId) ?? []),
  );
}

/** A Zapper's picture: its own image when it has a readable one, otherwise its mascot showing `mood`. */
export function Avatar(props: { avatar: ZapperAvatar; size?: number; image?: string; mood?: Mood }) {
  const [failed,setFailed]=createSignal(false);
  createEffect(()=>props.image,()=>{setFailed(false);});
  const image=()=>props.image && props.image.length<=96*1024 && /^data:image\/(png|jpeg);base64,[A-Za-z0-9+/=]+$/.test(props.image) && !failed() ? props.image : null;
  return <Show when={image()} fallback={<Mascot avatar={props.avatar} size={props.size ?? 20} mood={props.mood ?? "awake"} />}>{src=><img class="zp-avatar zp-avatar-image" src={src()} alt="" width={props.size ?? 20} height={props.size ?? 20} onError={()=>setFailed(true)} />}</Show>;
}

function Mascot(props: { avatar: ZapperAvatar; size: number; mood: Mood }) {
  let svg!: SVGSVGElement;
  const paths: SVGPathElement[] = [];
  const mascot = createMemo(() => mascots.find(m => m.id === props.avatar.mascot) ?? mascots[0]!);
  const color = createMemo(() => colors.find(c => c.id === props.avatar.color) ?? colors[0]!);
  let face: ReturnType<typeof attach> | undefined;
  onSettled(() => {
    face = attach(svg, paths, mascot(), props.size, props.mood);
    return () => face?.detach();
  });
  createEffect(() => [mascot(), props.size, props.mood] as const, ([next, size, mood]) => face?.update(next, size, mood));
  return (
    <svg ref={svg} class="zp-avatar zp-mascot" data-mood={props.mood} viewBox={`0 0 ${gridFor(props.size)} ${gridFor(props.size)}`} width={props.size} height={props.size} shape-rendering="crispEdges" aria-hidden="true">
      <path ref={el => (paths[0] = el)} fill={color().body} />
      <path ref={el => (paths[1] = el)} fill={EYE} />
    </svg>
  );
}
