import type { ZapperAvatar } from "@/core/zapper-types";
export interface ZapperSender {
  zapperId: string;
  chatId: string;
  name: string;
  avatar?: ZapperAvatar;
  activity?: boolean;
}
/** Display-only projection of the durable engine envelope. Original prompts stay intact. */
export function zapperMessage(
  text: string,
): { sender: ZapperSender; text: string } | null {
  const prefix = ["[Message from Zapper ", "[Message from Zapper "].find(prefix => text.startsWith(prefix));
  if (!prefix) return null;
  const end = text.indexOf("]\n\n", prefix.length);
  if (end < 0) return null;
  try {
    const sender = JSON.parse(text.slice(prefix.length, end)) as ZapperSender;
    if (
      typeof sender.name !== "string" ||
      !sender.name.trim() ||
      typeof sender.zapperId !== "string" ||
      !sender.zapperId ||
      typeof sender.chatId !== "string" ||
      !sender.chatId
    )
      return null;
    const body = text.slice(end + 3).split(/\n\n\[(?:Zapper|Zapper) execution context\]\n/)[0]!;
    return { sender, text: body };
  } catch {
    return null;
  }
}
