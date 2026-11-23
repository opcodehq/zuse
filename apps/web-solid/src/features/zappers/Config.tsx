import { ZapperModelPicker } from "./Controls";
import {
  createMemo,
  createSignal,
  For,
  Show,
  snapshot,
  untrack,
} from "solid-js";
import { state } from "@/core/store";
import { navigate, view } from "@/core/view";
import { uuid } from "@/core/util";
import type { Zapper, ZapperConfig } from "@/core/zapper-types";
import { Icon } from "@/ui/Icon";
import { Button } from "@/ui/primitives";
import { CommandAlert } from "./Alert";
import { Avatar, defaultAvatar } from "./Avatar";
import { mascots, colors } from "./mascots";
import { command, commandState, openZapper } from "./state";

export function ConfigPage() {
  const id = () =>
    view.route.kind === "newZapper" ? view.route.zapperId : undefined;
  const existing = () => state.zappers.find((z) => z.id === id());
  return (
    <Show
      when={(!id() && state.localDeviceId) || existing()}
      fallback={
        <div class="zp-empty">
          {id() ? "This Zapper no longer exists." : "Loading computers…"}
        </div>
      }
    >
      <For each={[id() ?? "new"]}>
        {(key) => (
          <ConfigForm existing={state.zappers.find((z) => z.id === key)} />
        )}
      </For>
    </Show>
  );
}
function ConfigForm(props: { existing?: Zapper }) {
  const id = untrack(() => props.existing?.id) ?? uuid();
  const initial = createMemo<ZapperConfig>(
    () =>
      props.existing?.config ?? {
        name: "",
        purpose: "",
        homeDeviceId: state.localDeviceId ?? "",
        repositories: [],
        coordinatorModel: {
          harness:
            state.harnesses.find((h) => h.id !== "mock")?.id ?? "claude-code",
          model: null,
        },
        workerDeviceId: state.localDeviceId ?? "",
        maxConcurrentWorkers: 4,
        maxFollowups: 3,
        avatar: defaultAvatar(id),
      },
  );
  const [draft, setDraft] = createSignal<ZapperConfig>(() =>
    untrack(() => structuredClone(snapshot(initial()))),
  );
  const [error, setError] = createSignal<string | null>(null);
  const change = <K extends keyof ZapperConfig>(
    key: K,
    value: ZapperConfig[K],
  ) => setDraft((d) => ({ ...d, [key]: value }));
  const avatar = () => draft().avatar ?? defaultAvatar(id);
  const uploadAvatar = async (file: File | undefined) => {
    if(!file) return;
    if(!["image/png","image/jpeg"].includes(file.type) || file.size>10*1024*1024) {setError("Choose a PNG or JPEG under 10 MB.");return;}
    let bitmap: ImageBitmap | undefined;
    try {
      bitmap=await createImageBitmap(file);
      const canvas=document.createElement("canvas");canvas.width=128;canvas.height=128;
      const context=canvas.getContext("2d");if(!context)throw new Error("Image preview is unavailable.");
      const edge=Math.min(bitmap.width,bitmap.height);
      context.drawImage(bitmap,(bitmap.width-edge)/2,(bitmap.height-edge)/2,edge,edge,0,0,128,128);
      const image=canvas.toDataURL("image/png");if(image.length>96*1024)throw new Error("This picture is too large. Try a smaller image.");
      change("avatarImage",image);setError(null);
    } catch {setError("Could not read this picture. Choose a different PNG or JPEG.");} finally {bitmap?.close();}
  };
  const devices = createMemo(() => Object.values(state.devices));
  const save = async (event: SubmitEvent) => {
    event.preventDefault();
    const config = { ...draft(), name: draft().name.trim() };
    if (!config.name) return void setError("Give this Zapper a name.");
    if (!config.homeDeviceId || !config.workerDeviceId)
      return void setError("Choose the coordinator and worker computers.");
    setError(null);
    try {
      const zapper = await command(
        id,
        { type: props.existing ? "configure" : "create", config },
        config.homeDeviceId,
      );
      if (zapper) {
        await Promise.resolve();
        openZapper(zapper);
      }
    } catch {
      /* The shared command alert retains the exact retry, if delivery is uncertain. */
    }
  };
  const cancel = () =>
    props.existing
      ? openZapper(props.existing)
      : navigate({ kind: "zappers", zapperId: "" });
  return (
    <div class="zp-config">
      <CommandAlert />
      <button type="button" class="zp-back" onClick={cancel}>
        <Icon name="alt-arrow-left" size={14} />
        Zappers
      </button>
      <h1>{props.existing ? "Zapper settings" : "New Zapper"}</h1>
      <p class="zp-muted">
        A Zapper is your captain: it runs work across your repositories, asks
        when it needs a decision and keeps its notes between conversations.
      </p>
      <form onSubmit={save} class="zp-form">
        <label>
          Name
          <input
            required
            value={draft().name}
            onInput={(e) => change("name", e.currentTarget.value)}
            placeholder="e.g. Release captain"
          />
        </label>
        <label>
          What this Zapper is responsible for
          <textarea
            rows={5}
            value={draft().purpose}
            onInput={(e) => change("purpose", e.currentTarget.value)}
            placeholder="Goals, responsibilities, and how you want it to work"
          />
        </label>
        <fieldset>
          <legend>Picture</legend><div class="zp-avatar-upload"><Avatar avatar={avatar()} image={draft().avatarImage} size={48} /><label class="zp-upload-button">Upload picture<input type="file" aria-label="Upload Zapper picture" accept="image/png,image/jpeg" onChange={event=>{ void uploadAvatar(event.currentTarget.files?.[0]);event.currentTarget.value=""; }} /></label><Show when={draft().avatarImage}><Button size="sm" onClick={()=>change("avatarImage",undefined)}>Use character</Button></Show></div><p class="zp-muted">PNG or JPEG. Cropped to a 128px square.</p>
          <h3>Character</h3>
          <div class="zp-avatar-options">
            <For each={mascots}>
              {(mascot) => (
                <button
                  type="button"
                  aria-label={mascot.name}
                  aria-pressed={
                    avatar().mascot === mascot.id ? "true" : "false"
                  }
                  onClick={() =>
                    change("avatar", { ...avatar(), mascot: mascot.id })
                  }
                >
                  <Avatar
                    avatar={{ ...avatar(), mascot: mascot.id }}
                    size={32}
                  />
                </button>
              )}
            </For>
          </div>
          <div class="zp-color-options">
            <For each={colors}>
              {(color) => (
                <button
                  type="button"
                  aria-label={`${color.id} character`}
                  aria-pressed={avatar().color === color.id ? "true" : "false"}
                  style={{ "background-color": color.body }}
                  onClick={() =>
                    change("avatar", { ...avatar(), color: color.id })
                  }
                />
              )}
            </For>
          </div>
        </fieldset>
        <div class="zp-form-grid">
          <label>
            Coordinator computer
            <select
              disabled={!!props.existing}
              value={draft().homeDeviceId}
              onChange={(e) => change("homeDeviceId", e.currentTarget.value)}
            >
              <Show when={!devices().some((d) => d.id === state.localDeviceId)}>
                <option value={state.localDeviceId ?? ""}>This computer</option>
              </Show>
              <For each={devices()}>
                {(device) => (
                  <option value={device.id}>{device.name ?? device.id}</option>
                )}
              </For>
            </select>
          </label>
          <label>
            Worker computer
            <select
              value={draft().workerDeviceId}
              onChange={(e) => change("workerDeviceId", e.currentTarget.value)}
            >
              <Show when={!devices().some((d) => d.id === state.localDeviceId)}>
                <option value={state.localDeviceId ?? ""}>This computer</option>
              </Show>
              <For each={devices()}>
                {(device) => (
                  <option value={device.id}>{device.name ?? device.id}</option>
                )}
              </For>
            </select>
          </label>
        </div>
        <fieldset>
          <legend>Repositories</legend>
          <p class="zp-muted">
            Threads can work in the repositories you switch on.
          </p>
          <For each={Object.values(state.spaces)}>
            {(space) => (
              <label class="zp-check">
                <input
                  type="checkbox"
                  checked={draft().repositories.some(
                    (r) =>
                      r.spaceId === space.id && r.deviceId === space.deviceId,
                  )}
                  onChange={(e) =>
                    change(
                      "repositories",
                      e.currentTarget.checked
                        ? [
                            ...draft().repositories,
                            { spaceId: space.id, deviceId: space.deviceId },
                          ]
                        : draft().repositories.filter(
                            (r) => r.spaceId !== space.id,
                          ),
                    )
                  }
                />
                <span>
                  {space.name || space.path}
                  <small>
                    {space.path} ·{" "}
                    {state.devices[space.deviceId]?.name ?? "This computer"}
                  </small>
                </span>
              </label>
            )}
          </For>
          <Show when={!Object.keys(state.spaces).length}>
            <p class="zp-muted">
              Add a project to make a repository available here.
            </p>
          </Show>
        </fieldset>
        <div class="zp-form-grid">
          <label>Coordinator model<ZapperModelPicker label="Coordinator model" deviceId={draft().homeDeviceId} value={draft().coordinatorModel} onChange={value => change("coordinatorModel",value)} /></label>
          <label>
            Concurrent threads
            <input
              type="number"
              min="1"
              max="32"
              required
              value={draft().maxConcurrentWorkers}
              onInput={(e) =>
                change("maxConcurrentWorkers", e.currentTarget.valueAsNumber)
              }
            />
          </label>
          <label>
            Automatic follow-ups
            <input
              type="number"
              min="0"
              max="32"
              required
              value={draft().maxFollowups}
              onInput={(e) =>
                change("maxFollowups", e.currentTarget.valueAsNumber)
              }
            />
          </label>
        </div>
        <Show when={error()}>
          <p class="zp-error" role="alert">
            {error()}
          </p>
        </Show>
        <Show when={commandState.error}>
          <p class="zp-error" role="alert">
            {commandState.error}
          </p>
        </Show>
        <div class="zp-form-actions">
          <Button type="button" onClick={cancel}>
            Cancel
          </Button>
          <Button
            type="submit"
            variant="primary"
            disabled={
              commandState.busy || state.connection.kind !== "connected"
            }
          >
            {commandState.busy
              ? "Saving…"
              : props.existing
                ? "Save changes"
                : "Create Zapper"}
          </Button>
        </div>
      </form>
    </div>
  );
}
