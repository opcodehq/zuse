import { OwnerTransport, type Transport } from "@/core/transport";
import { AccountTransport } from "../account";
import { linkFor } from "../computers";
import { PairedTransport } from "../paired";
import { type Recipe, stagedPairing, validRecipe } from "./model";
export function computerTransport(recipe: Recipe): Transport {
  if (!validRecipe(recipe)) throw new Error("Invalid saved computer connection");
  if (recipe.kind === "owner") return new OwnerTransport();
  if (recipe.kind === "paired") {
    const link = linkFor(recipe.pairId) ?? stagedPairing(recipe.pairId);
    if (!link) throw new Error("Pair this computer again to reconnect.");
    return new PairedTransport(link);
  }
  return new AccountTransport(recipe.target, () => computerTransport(recipe.controller));
}
