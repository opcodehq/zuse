import { createMemo, For, Show } from "solid-js";
import { state } from "@/core/store";
import { navigate, openChat, setView, view } from "@/core/view";
import { zapperForChat } from "@/core/zappers";
import { Icon } from "@/ui/Icon";
import { Menu } from "@/ui/primitives";
import { Avatar, avatarOf } from "./Avatar";
import {
  act,
  openZapper,
  statusLabel,
  threadActive,
  threadTitle,
  workerVisible,
} from "./state";
import "./zappers.css";

export function ZapperSidebar() {
  const zappers = createMemo(() =>
    state.zappers.filter((z) => z.status !== "archived"),
  );
  const owner = () => zapperForChat(view.selectedChatId);
  return (
    <Show when={state.zappersSupported !== false}>
      <section class="zp-sidebar" aria-label="Zappers">
        <div class="zp-sidebar-heading">
          <button
            type="button"
            onClick={() => {
              setView((d) => {
                d.selectedChatId = null;
              });
              navigate({ kind: "zappers", zapperId: "" });
            }}
          >
            Zappers
          </button>
          <button
            type="button"
            class="zp-icon"
            aria-label="New Zapper"
            title="New Zapper"
            onClick={() => navigate({ kind: "newZapper" })}
          >
            <Icon name="plus" size={13} />
          </button>
        </div>
        <Show when={state.zappersError}>
          <p class="zp-error">Zappers unavailable: {state.zappersError}</p>
        </Show>
        <For each={zappers()}>
          {(zot) => {
            const waiting = () =>
              zot.decisions.filter((d) => d.answer === null).length;
            const workers = createMemo(() =>
              zot.workers.filter(
                (w) => workerVisible(w.chatId) && threadActive(w.chatId),
              ),
            );
            return (
              <div class="zp-sidebar-group" data-zapper-id={zot.id}>
                <div
                  class="zp-sidebar-row"
                  data-selected={owner()?.id === zot.id ? "true" : undefined}
                >
                  <button
                    type="button"
                    class="zp-sidebar-open"
                    aria-label={`Open Zapper ${zot.config.name}`}
                    onClick={() => openZapper(zot)}
                  >
                    <Avatar avatar={avatarOf(zot)} size={17} />
                    <span>{zot.config.name}</span>
                    <Show when={waiting()}>
                      <span class="zp-badge" title="Decisions waiting for you">
                        {waiting()}
                      </span>
                    </Show>
                    <Show when={zot.status !== "active"}>
                      <span
                        class="zp-status-dot"
                        title={statusLabel(zot.status)}
                      />
                    </Show>
                  </button>
                  <Menu
                    trigger={(attrs) => (
                      <button
                        {...attrs}
                        type="button"
                        class="zp-icon zp-sidebar-more"
                        aria-label={`Actions for ${zot.config.name}`}
                      >
                        <Icon name="menu-dots" size={13} />
                      </button>
                    )}
                    items={[
                      {
                        label: "Zapper settings",
                        icon: "settings",
                        onSelect: () =>
                          navigate({ kind: "newZapper", zapperId: zot.id }),
                      },
                      {
                        label: zot.status === "active" ? "Pause" : "Resume",
                        onSelect: () =>
                          act(zot.id, {
                            type: "setStatus",
                            status:
                              zot.status === "active" ? "paused" : "active",
                          }),
                      },
                      { label: "Open Zapper", onSelect: () => openZapper(zot) },
                    ]}
                  />
                </div>
                <For each={workers()}>
                  {(worker) => (
                    <div class="zp-sidebar-worker">
                      <button
                        type="button"
                        data-selected={
                          view.selectedChatId === worker.chatId
                            ? "true"
                            : undefined
                        }
                        onClick={() => openChat(worker.chatId)}
                      >
                        <Icon
                          name={
                            state.sessions[worker.chatId]?.status === "working"
                              ? "clock-circle"
                              : "git-branch"
                          }
                          size={12}
                        />
                        <span class="zp-sidebar-worker-label">
                          {threadTitle(worker.chatId, worker.objective)}
                        </span>
                      </button>
                      <button
                        type="button"
                        class="zp-icon"
                        aria-label={`Archive ${threadTitle(worker.chatId, worker.objective)}`}
                        onClick={() =>
                          act(zot.id, {
                            type: "archiveWorker",
                            chatId: worker.chatId,
                          })
                        }
                      >
                        <Icon name="archive-minimalistic" size={12} />
                      </button>
                    </div>
                  )}
                </For>
              </div>
            );
          }}
        </For>
        <Show when={state.zappersReady && !zappers().length}>
          <button
            type="button"
            class="zp-sidebar-empty"
            onClick={() => navigate({ kind: "newZapper" })}
          >
            Create your first Zapper
          </button>
        </Show>
      </section>
    </Show>
  );
}
