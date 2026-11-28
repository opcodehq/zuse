import { afterEach, test } from "node:test";
import assert from "node:assert/strict";
import { openAccountComputer } from "./account";
import { registerComputerAdder } from "./multi/navigation";

const originals = new Map<string, PropertyDescriptor | undefined>();
function replaceGlobal(name: string, value: unknown) {
  originals.set(name, Object.getOwnPropertyDescriptor(globalThis, name));
  Object.defineProperty(globalThis, name, { configurable: true, value });
}
afterEach(() => {
  registerComputerAdder(null);
  for (const [name, descriptor] of originals) {
    if (descriptor) Object.defineProperty(globalThis, name, descriptor);
    else Reflect.deleteProperty(globalThis, name);
  }
  originals.clear();
});

test("connecting to an account computer reloads this tab without opening a window", () => {
  const stored = new Map<string, string>();
  const urls: string[] = [];
  let reloads = 0;
  replaceGlobal("sessionStorage", {
    setItem: (key: string, value: string) => stored.set(key, value),
    getItem: (key: string) => stored.get(key) ?? null,
  });
  replaceGlobal("location", { pathname: "/app", search: "?workspace=one", reload: () => reloads++ });
  replaceGlobal("history", { replaceState: (_state: unknown, _title: string, url: string) => urls.push(url) });
  replaceGlobal("window", { open: () => { throw new Error("Must stay in the current tab"); } });
  const target = { environmentId: "remote", expectedSubject: "user", label: "MacBook" };
  openAccountComputer(target);
  assert.equal(reloads, 1);
  assert.deepEqual(urls, [`/app?workspace=one#computer=${encodeURIComponent(JSON.stringify(target))}`]);
  assert.deepEqual(JSON.parse(stored.get("zuse.account-target.v1")!), target);
  assert.throws(() => openAccountComputer({ ...target, environmentId: "" }));
  assert.equal(reloads, 1);
});

test("the shared sidebar adds an account computer without reloading or replacing the current workspace", () => {
  let reloads = 0;
  const requests: unknown[] = [];
  replaceGlobal("location", { pathname: "/app", search: "", reload: () => reloads++ });
  replaceGlobal("window", { open: () => { throw new Error("Must not open a new window"); } });
  registerComputerAdder(request => requests.push(request));
  const target = { environmentId: "server", expectedSubject: "account", label: "Server" };
  openAccountComputer(target);
  assert.deepEqual(requests, [{ kind: "account", target }]);
  assert.equal(reloads, 0);
});
