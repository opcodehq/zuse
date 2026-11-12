import { toast } from "@/ui/primitives";
import { writeClipboard } from "@/ui/clipboard";

export async function copyText(text: string): Promise<boolean> {
  const ok = await writeClipboard(text);
  if (!ok) toast("Couldn't copy to the clipboard.", "danger");
  return ok;
}
