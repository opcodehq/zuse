// App actions the palette and shortcuts run. Mutations go to the engine; nothing is applied locally first.
import { rpc } from "@/core/connection";
import { chatRequest } from "@/core/chat";
import { M } from "@/core/methods";
import { RpcError } from "@/core/rpc";
import { state } from "@/core/store";
import { view } from "@/core/view";
import { openFileInPanel } from "@/features/workspace/files/model";
import { toast } from "@/ui/primitives";

/** The selected chat, when it exists and can still be archived. */
export function archivableChat() {
  const id = view.route.kind === "chat" ? view.selectedChatId : null;
  const chat = id ? state.chats[id] : undefined;
  return chat && !chat.archived ? chat : undefined;
}

export async function archiveSelectedChat() {
  const chat = archivableChat();
  if (!chat) return;
  try {
    await rpc.call(M.MUTATE, chatRequest(chat.id, { op: "setChatArchived", archived: true }));
    toast("Thread archived");
  } catch (error) {
    toast(
      error instanceof RpcError && error.outcomeUnknown
        ? "The connection dropped while archiving. The thread may already be archived; check before trying again."
        : `Couldn't archive the thread. ${error instanceof Error ? error.message : ""}`.trim(),
      "danger",
    );
  }
}

/** Open a path of the selected chat in the workspace's Files panel, which reveals and opens it. */
export function openWorkspacePath(path: string) {
  openFileInPanel(path);
}
