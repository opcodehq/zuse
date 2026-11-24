import { test } from "node:test";
import assert from "node:assert/strict";
import { nextThread, projectGroups, recipeKey, validRecipe, type Computer, type Recipe } from "./model";
import type { Chat, Space } from "@/core/protocol";
const space = (computer: string, repositoryId: string | null = "commit:shared"): Space => ({ id: "project", deviceId: computer, path: `/${computer}/project`, name: "Project", repositoryId, gitDetected: true, createdAt: "2026-01-01T00:00:00Z" });
const chat = (computer: string, title: string): Chat => ({ id: "same-chat-id", spaceId: "project", deviceId: computer, title, archived: false, createdAt: "2026-01-01T00:00:00Z" });
const computer = (id: string, title: string): Computer => ({ id, label: id, recipe: { kind: "owner" }, snapshot: { deviceId: id, label: id, connected: true, synced: true, selected: "same-chat-id", spaces: [space(id)], chats: [chat(id, title)], sessions: {}, dirty: false } });
test("one repository group keeps every checkout and its owning computer", () => {
  const groups = projectGroups([computer("macbook", "Fix login"), computer("server", "Deploy service")]);
  assert.equal(groups.length, 1);
  assert.deepEqual(groups[0]!.parts.map(part => part.computer.id), ["macbook", "server"]);
  assert.deepEqual(groups[0]!.parts.map(part => part.chats[0]!.title), ["Fix login", "Deploy service"]);
  assert.equal(projectGroups([computer("macbook", "Fix login"), computer("server", "Deploy service")], "server deploy")[0]!.parts[0]!.computer.id, "server");
});
test("project ids alone never merge unrelated computers and empty offline checkouts remain visible", () => {
  const a = computer("macbook", "Local"); const b = computer("server", "Remote");
  a.snapshot!.spaces = [space("macbook", null)]; b.snapshot!.spaces = [space("server", null)];
  b.snapshot!.connected = false; b.snapshot!.chats = [];
  const groups = projectGroups([a, b]);
  assert.equal(groups.length, 2);
  assert.equal(groups[1]!.parts[0]!.computer.snapshot!.connected, false);
  assert.equal(groups[1]!.parts[0]!.chats.length, 0);
});
test("search excludes archived and foreign-device rows without confusing duplicate chat ids", () => {
  const a = computer("macbook", "Fix login");
  a.snapshot!.chats.push({ ...chat("server", "Foreign"), id: "foreign" }, { ...chat("macbook", "Archived"), id: "archived", archived: true });
  assert.deepEqual(projectGroups([a])[0]!.parts[0]!.chats.map(chat => chat.id), ["same-chat-id"]);
  assert.deepEqual(projectGroups([a], "foreign"), []);
  const order = [{ computer: "macbook", chat: "same-chat-id" }, { computer: "server", chat: "same-chat-id" }];
  assert.deepEqual(nextThread(order, order[0]!, true), order[1]);
  assert.deepEqual(nextThread(order, order[1]!, true), order[0]);
  assert.deepEqual(nextThread(order, order[0]!, false), order[1]);
  assert.equal(nextThread([], null, true), undefined);
});
test("saved recipes fence controller depth and carry public metadata instead of pairing capabilities", () => {
  const paired: Recipe = { kind: "paired", pairId: "a".repeat(32) };
  const target = { environmentId: "server", expectedSubject: "account", label: "Server" };
  const account: Recipe = { kind: "account", target, controller: paired };
  assert.equal(validRecipe(account), true);
  assert.equal(recipeKey(account), "account:account:server");
  assert.equal(JSON.stringify(account).includes("#pair="), false);
  assert.equal(validRecipe({ kind: "paired", pairId: "bad" }), false);
  assert.equal(validRecipe({ ...account, target: { ...target, expectedSubject: "" } }), false);
  let chain: Recipe = account;
  for (let index = 0; index < 4; index++) chain = { kind: "account", target, controller: chain };
  assert.equal(validRecipe(chain), false);
});
