import "../web-parity/fixture";
import { flush } from "solid-js";
import { setState } from "@/core/store";
import { setView, view } from "@/core/view";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";

const subscribe = rpc.subscribe.bind(rpc);
rpc.subscribe = ((method, params, onItem, onError) => {
  if (method !== M.WATCH_DOC_MESSAGES || (params as { chatId: string }).chatId !== "linked") return subscribe(method, params, onItem, onError);
  queueMicrotask(() => onItem({ reset: [{ id: "reference", deviceId: "remote", role: "assistant", createdAt: 0, parts: [{ kind: "text", id: "text", text: "Inspect ZAP-12 from this linked conversation." }] }] } as never));
  return { cancel() {} };
}) as typeof rpc.subscribe;

Object.assign(window, { referenceNavigation: {
  start: () => {
    const parity = (window as unknown as { parity: { workspace(): void; zapper(): void } }).parity;
    parity.workspace();
    parity.zapper();
    setState(draft => {
      draft.chats.linked = { id: "linked", deviceId: "remote", title: "Linked normal conversation", archived: false, createdAt: "", cwd: "/workspace/zuse", spaceId: "project" };
      draft.zappers[0]!.issues[0]!.threads.push("linked");
    });
    setView(draft => { draft.route = { kind: "chat" }; draft.selectedChatId = "linked"; draft.rightPanel = "files"; });
    flush();
  },
  state: () => ({ route: { kind: view.route.kind, zapperId: view.route.kind === "zappers" ? view.route.zapperId : undefined }, chat: view.selectedChatId, rightPanel: view.rightPanel }),
  coverInspector: () => { setView(draft => { draft.rightPanel = "files"; }); flush(); },
} });
