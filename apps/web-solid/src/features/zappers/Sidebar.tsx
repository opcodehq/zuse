import { workerRank, workerStatus } from "./threads-model";
import { createMemo, For, Show } from "solid-js";
import { state } from "@/core/store";
import { navigate, openChat, setView, view } from "@/core/view";
import { zapperForChat } from "@/core/zappers";
import { Icon } from "@/ui/Icon";
import { Menu } from "@/ui/primitives";
import { Avatar, avatarOf, moodOf } from "./Avatar";
import { moodLabel } from "./mascot-motion";
import { now } from "@/features/sidebar/util";
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
          {(zapper) => {
            const waiting = () =>
              zapper.decisions.filter((d) => d.answer === null).length;
            const mood = createMemo(() => moodOf(zapper, now()));
            const workers = createMemo(() =>
              zapper.workers.filter(
                (w) => workerVisible(w.chatId) && (w.queuePosition !== undefined || threadActive(w.chatId)),
              ).sort((a,b)=>workerRank(a)-workerRank(b)||(a.queuePosition ?? 0)-(b.queuePosition ?? 0)||a.chatId.localeCompare(b.chatId)),
            );
            return (
              <div class="zp-sidebar-group" data-zapper-id={zapper.id}>
                <div
                  class="zp-sidebar-row"
                  data-selected={owner()?.id === zapper.id ? "true" : undefined}
                >
                  <button
                    type="button"
                    class="zp-sidebar-open"
                    aria-label={`Open Zapper ${zapper.config.name}`}
                    aria-describedby={`zp-mood-${zapper.id}`}
                    title={moodLabel[mood()]}
                    onClick={() => openZapper(zapper)}
                  >
                    <Avatar avatar={avatarOf(zapper)} image={zapper.config.avatarImage} size={17} mood={mood()} />
                    <span id={`zp-mood-${zapper.id}`} hidden>{moodLabel[mood()]}</span>
                    <span>{zapper.config.name}</span>
                    <Show when={waiting()}>
                      <span class="zp-badge" title="Decisions waiting for you">
                        {waiting()}
                      </span>
                    </Show>
                    <Show when={zapper.status !== "active"}>
                      <span
                        class="zp-status-dot"
                        title={statusLabel(zapper.status)}
                      />
                    </Show>
                  </button>
                  <Menu
                    trigger={(attrs) => (
                      <button
                        {...attrs}
                        type="button"
                        class="zp-icon zp-sidebar-more"
                        aria-label={`Actions for ${zapper.config.name}`}
                      >
                        <Icon name="menu-dots" size={13} />
                      </button>
                    )}
                    items={[
                      {
                        label: "Zapper settings",
                        icon: "settings",
                        onSelect: () =>
                          navigate({ kind: "newZapper", zapperId: zapper.id }),
                      },
                      {
                        label: zapper.status === "active" ? "Pause" : "Resume",
                        onSelect: () =>
                          act(zapper.id, {
                            type: "setStatus",
                            status:
                              zapper.status === "active" ? "paused" : "active",
                          }),
                      },
                      { label: "Open Zapper", onSelect: () => openZapper(zapper) },
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
                        title={workerStatus(worker)}
                        onClick={() => openChat(worker.chatId)}
                      >
                        <Icon
                          name={
                            worker.queuePosition !== undefined || state.sessions[worker.chatId]?.status === "working"
                              ? "clock-circle"
                              : "git-branch"
                          }
                          size={12}
                        />
                        <span class="zp-sidebar-worker-label">
                          {threadTitle(worker.chatId, worker.objective)}
                        </span><Show when={worker.queuePosition !== undefined}><small class="zp-queued-badge">Queued</small></Show>
                      </button>
                      <button
                        type="button"
                        class="zp-icon"
                        aria-label={`Archive ${threadTitle(worker.chatId, worker.objective)}`}
                        onClick={() =>
                          act(zapper.id, {
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
