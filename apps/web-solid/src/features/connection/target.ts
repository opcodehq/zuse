// Chooses what this tab connects to before the connection loop starts. A `#pair=` capability is
// removed from the address bar immediately, before anything else can read or record it.
import { checkSession } from "@/core/auth";
import { ConnectionBlocked, setTransportFactory } from "@/core/connection";
import { OwnerTransport, type Transport } from "@/core/transport";
import { AccountTransport, takeAccountTarget } from "./account";
import { describeLink, latest, publish, selectLink, selectedLink, writeAccountTarget } from "./computers";
import { loadLink } from "./link/wasm";
import { PairedTransport } from "./paired";
import { setArea, type AccountTarget } from "./status";

type Choice =
  | { kind: "owner" }
  | { kind: "paired"; link: string }
  | { kind: "fragment"; origin: string; fragment: string }
  | { kind: "resume"; link: string }
  | { kind: "invalid"; status: ConnectionBlocked["status"] };

function clearAddress() {
  history.replaceState(null, "", location.pathname + location.search);
}

function choose(): Choice {
  const hash = location.hash;
  if (hash.startsWith("#token=")) {
    selectLink(null);
    writeAccountTarget(null);
    return { kind: "owner" };
  }
  if (hash.startsWith("#pair=")) {
    // Remove the capability even when its contents are invalid.
    clearAddress();
    selectLink(null);
    writeAccountTarget(null);
    return { kind: "fragment", origin: location.origin, fragment: hash };
  }
  const link = selectedLink();
  if (link) {
    return describeLink(link)
      ? { kind: "paired", link }
      : { kind: "invalid", status: { kind: "pairingRequired", message: "Saved pairing is invalid. Forget it and open a new invitation." } };
  }
  const saved = latest();
  return saved ? { kind: "resume", link: saved } : { kind: "owner" };
}

/** The controller an account tab authenticates through: this tab's pairing, or the page's own engine. */
function controllerFactory(choice: Choice): () => Transport | Promise<Transport> {
  if (choice.kind === "paired") return () => new PairedTransport(choice.link);
  return () => new OwnerTransport();
}

function usePaired(link: string) {
  setArea({ mode: "paired", origin: describeLink(link)?.origin ?? location.origin });
  setTransportFactory(() => new PairedTransport(link), { ownerSession: false });
}

function useAccount(target: AccountTarget, controller: () => Transport | Promise<Transport>) {
  setArea({ mode: "account", account: target, origin: "" });
  setTransportFactory(
    () => {
      const transport = new AccountTransport(target, controller);
      const open = transport.open.bind(transport);
      transport.open = async () => {
        await open();
        setArea("origin", transport.origin);
      };
      return transport;
    },
    { ownerSession: false },
  );
}

function install() {
  const choice = choose();
  let account: AccountTarget | null = null;
  try {
    account = takeAccountTarget();
  } catch (error) {
    // Decoder and parser errors describe JSON, not the link.
    const message =
      error instanceof Error && error.message.startsWith("Return to the controller")
        ? error.message
        : "This computer link is invalid. Open the computer again from your controller.";
    setArea({ mode: "account" });
    setTransportFactory(() => Promise.reject(new ConnectionBlocked({ kind: "accountRequired", message })), { ownerSession: false });
    return;
  }
  if (account && (choice.kind === "owner" || choice.kind === "paired")) {
    useAccount(account, controllerFactory(choice));
    return;
  }
  switch (choice.kind) {
    case "owner":
      return;
    case "paired":
      return usePaired(choice.link);
    case "invalid":
      setArea({ mode: "paired" });
      setTransportFactory(() => Promise.reject(new ConnectionBlocked(choice.status)), { ownerSession: false });
      return;
    case "fragment": {
      setArea({ mode: "paired", origin: choice.origin });
      let link: string | null = null;
      setTransportFactory(
        async () => {
          if (!link) {
            const module = await loadLink();
            try {
              link = module.linkFromFragment(choice.origin, choice.fragment);
            } catch (error) {
              const detail = error instanceof Error ? error.message.replace(/\.?$/, ". ") : "";
              throw new ConnectionBlocked({ kind: "pairingRequired", message: `${detail}Open a new invitation from the other computer.` });
            }
            selectLink(link);
            publish();
            usePaired(link);
          }
          return new PairedTransport(link);
        },
        { ownerSession: false },
      );
      return;
    }
    case "resume": {
      // An owner session wins. Otherwise a new tab reopens the last computer.
      setTransportFactory(
        async () => {
          if ((await checkSession()) === "owner") {
            setTransportFactory(() => new OwnerTransport());
            return new OwnerTransport();
          }
          selectLink(choice.link);
          publish();
          usePaired(choice.link);
          return new PairedTransport(choice.link);
        },
        { ownerSession: false },
      );
      return;
    }
  }
}

install();
