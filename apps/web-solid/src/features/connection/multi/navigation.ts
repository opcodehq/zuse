import type { Recipe } from "./model";
import { frameId, inComputerFrame } from "./model";
export type AddRequest = { kind: "paired"; link: string } | { kind: "account"; target: Extract<Recipe, { kind: "account" }>["target"] };
let addHandler: ((request: AddRequest) => void) | null = null;
export function registerComputerAdder(handler: ((request: AddRequest) => void) | null) { addHandler = handler; }
export function openComputer(request: AddRequest): boolean {
  if (inComputerFrame()) { parent.postMessage({ type: "zuse.computer.add", frame: frameId(), request }, location.origin); return true; }
  if (!addHandler) return false;
  addHandler(request); return true;
}
