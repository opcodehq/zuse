// Confirmations and name prompts opened from sidebar menus.
import { createSignal, Match, Switch } from "solid-js";
import { state } from "@/core/store";
import { Button, Dialog, TextField } from "@/ui/primitives";
import { deleteChat, removeProject, renameProject } from "./actions";
import { deviceName, spaceName } from "./model";
import { dialog, setDialog, type SidebarDialog, titleOf } from "./overlay";
import { changeSection, sections } from "./pins";
import { singleLine, uuid } from "./util";

const close = () => setDialog(null);

function opened<K extends SidebarDialog["kind"]>(kind: K) {
  const current = dialog();
  return current?.kind === kind ? (current as Extract<SidebarDialog, { kind: K }>) : null;
}

function NamePrompt(props: { title: string; label: string; placeholder: string; initial: string; submit: string; onSubmit: (name: string) => boolean | void; allowEmpty?: boolean }) {
  const [value, setValue] = createSignal(props.initial);
  const submit = (event: Event) => {
    event.preventDefault();
    const name = value().trim();
    if (!name && !props.allowEmpty) return;
    if (props.onSubmit(name) !== false) close();
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => !open && close()}
      title={props.title}
      footer={
        <>
          <Button variant="ghost" onClick={close}>Cancel</Button>
          <Button variant="primary" type="submit" form="sb-name-form" disabled={!value().trim() && !props.allowEmpty}>{props.submit}</Button>
        </>
      }
    >
      <form id="sb-name-form" class="sb-dialog-field" onSubmit={submit}>
        <TextField aria-label={props.label} placeholder={props.placeholder} value={value()} onInput={(e) => setValue(e.currentTarget.value)} autofocus />
      </form>
    </Dialog>
  );
}

function Confirm(props: { title: string; body: string; action: string; onConfirm: () => void }) {
  return (
    <Dialog
      open
      onOpenChange={(open) => !open && close()}
      title={props.title}
      description={props.body}
      footer={
        <>
          <Button variant="ghost" onClick={close}>Cancel</Button>
          <Button
            variant="danger"
            onClick={() => {
              props.onConfirm();
              close();
            }}
          >
            {props.action}
          </Button>
        </>
      }
    />
  );
}

export function Dialogs() {
  return (
    <Switch>
      <Match when={opened("deleteChat")}>
        {(d) => {
          const chat = state.chats[d().chatId];
          const title = singleLine(chat ? titleOf(chat) : "") || "New session";
          return <Confirm title="Delete session?" body={`“${title}” will be permanently deleted. This can’t be undone.`} action="Delete" onConfirm={() => deleteChat(d().chatId)} />;
        }}
      </Match>
      <Match when={opened("removeProject")}>
        {(d) => {
          const space = state.spaces[d().spaceId];
          const name = space ? spaceName(space) : "this project";
          const device = (space && deviceName(space.deviceId)) || "its device";
          const count = Object.values(state.chats).filter((chat) => chat.spaceId === d().spaceId).length;
          const body = `Removing “${name}” permanently deletes its ${count} ${count === 1 ? "session" : "sessions"} on ${device}. This can’t be undone.`;
          return <Confirm title="Remove project?" body={body} action="Remove" onConfirm={() => removeProject(d().spaceId)} />;
        }}
      </Match>
      <Match when={opened("renameProject")}>
        {(d) => {
          const space = state.spaces[d().spaceId];
          return (
            <NamePrompt
              title="Rename project"
              label="Project name"
              placeholder={space ? spaceName({ ...space, name: null }) : "Project name"}
              initial={space?.name?.trim() || (space ? spaceName(space) : "")}
              submit="Rename"
              allowEmpty
              onSubmit={(name) => renameProject(d().spaceId, name)}
            />
          );
        }}
      </Match>
      <Match when={opened("section")}>
        {(d) => {
          const existing = d().sectionId ? sections().find((s) => s.id === d().sectionId) : undefined;
          return (
            <NamePrompt
              title={existing ? "Edit section" : "New section"}
              label="Section name"
              placeholder="Section name"
              initial={existing?.name ?? ""}
              submit={existing ? "Save" : "Create section"}
              onSubmit={(name) => {
                if (existing) {
                  if (name !== existing.name) changeSection({ action: "rename", id: existing.id, name });
                  return;
                }
                const id = uuid();
                changeSection({ action: "create", id, name });
                const assign = d().assign;
                if (assign) changeSection({ action: "assign", sessionId: assign, sectionId: id });
              }}
            />
          );
        }}
      </Match>
    </Switch>
  );
}
