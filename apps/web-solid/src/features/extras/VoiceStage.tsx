// The voice call's stage (orb, live caption, call bar) and the compact pill while it is hidden.
import { createEffect, Match, on, Switch } from "solid-js";
import { narrowLayout, sidebarVisible, view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { Button, IconButton } from "@/ui/primitives";
import { dismissVoiceNotice, endVoice, orbState, setVoiceStageOpen, toggleVoiceMute, voice } from "./voice";

const CAPTION_CHARS = 160;

function statusLabel() {
  const v = voice.view;
  if (!v) return "";
  if (v.step !== "active") return "Connecting voice…";
  if (v.muted) return "Microphone muted";
  if (v.snapshot?.playing) return "Speaking";
  if (v.snapshot?.work === "working") return "Working";
  if (v.snapshot?.work === "awaitingInput") return "Waiting on your input";
  return "Listening";
}

export function VoiceStage() {
  let stage: HTMLDivElement | undefined;
  let returnFocus: HTMLElement | null = null;
  createEffect(
    on(
      () => voice.stageOpen && !!voice.view,
      (open, wasOpen) => {
        if (open) {
          // Read in the microtask: an overlay that launched the call returns its focus first.
          queueMicrotask(() => {
            const active = document.activeElement;
            const outside = active instanceof HTMLElement && active !== document.body && !stage?.contains(active) && !active.closest(".x-voice-pill");
            if (!wasOpen && outside && !returnFocus?.isConnected) returnFocus = active;
            stage?.focus();
          });
        } else if (wasOpen) {
          restoreFocus();
        }
      },
    ),
  );
  // The stage or pill held focus; give it back instead of dropping it on the body. Hiding keeps the origin for the call's end.
  function restoreFocus() {
    const target = returnFocus;
    queueMicrotask(() => {
      if (document.activeElement && document.activeElement !== document.body) return;
      if (target?.isConnected) target.focus();
    });
  }
  createEffect(
    on(
      () => !!voice.view,
      (live, wasLive) => {
        if (live || !wasLive) return;
        restoreFocus();
        returnFocus = null;
      },
    ),
  );
  // Desktop keeps the sidebar usable beside the stage; collapsed, the stage spans the window.
  const left = () => (sidebarVisible() && !narrowLayout() ? `${view.preferences.sidebarWidth}px` : "0px");
  // The pill centres over the main pane, not the whole window.
  const centre = () => `calc(${left()} + (100vw - ${left()}) / 2)`;
  const level = () => {
    const v = voice.view;
    if (!v) return 0;
    return Math.max(v.muted ? 0 : v.microphone, v.speaker) / 65535;
  };
  const caption = () => {
    const text = voice.view?.caption ?? "";
    return [...text].slice(-CAPTION_CHARS).join("");
  };

  return (
    <Switch>
      <Match when={voice.view && voice.stageOpen}>
        <div
          ref={stage}
          class="x-voice-stage"
          style={{ left: left() }}
          role="dialog"
          aria-label="Voice call"
          tabIndex={-1}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              setVoiceStageOpen(false);
            }
          }}
        >
          <div class="x-voice-status" role="status" aria-live="polite">{statusLabel()}</div>
          <div class="x-orb" data-state={orbState(voice.view!)} style={{ "--level": String(level()) }} aria-hidden="true">
            <span class="x-orb-core" />
          </div>
          <p class="x-voice-caption" aria-live="polite">{caption()}</p>
          <div class="x-voice-bar" role="toolbar" aria-label="Call controls">
            <IconButton
              class="x-voice-control"
              icon={voice.view!.muted ? "microphone-off" : "microphone"}
              label={voice.view!.muted ? "Unmute" : "Mute"}
              active={voice.view!.muted}
              aria-pressed={voice.view!.muted}
              onClick={() => toggleVoiceMute()}
            />
            <IconButton class="x-voice-control" icon="alt-arrow-down" label="Hide voice call" onClick={() => setVoiceStageOpen(false)} />
            <IconButton class="x-voice-control" data-tone="danger" icon="phone-hang-up" label="End call" onClick={() => endVoice()} />
          </div>
        </div>
      </Match>
      <Match when={voice.view && !voice.stageOpen}>
        <div class="x-voice-pill" style={{ left: centre() }} role="region" aria-label="Voice call">
          <span class="x-orb x-orb-mini" data-state={orbState(voice.view!)} style={{ "--level": String(level()) }} aria-hidden="true">
            <span class="x-orb-core" />
          </span>
          <span class="x-voice-pill-label">{statusLabel()}</span>
          <IconButton size="sm" icon={voice.view!.muted ? "microphone-off" : "microphone"} label={voice.view!.muted ? "Unmute" : "Mute"} aria-pressed={voice.view!.muted} onClick={() => toggleVoiceMute()} />
          <IconButton size="sm" icon="alt-arrow-up" label="Show voice call" onClick={() => setVoiceStageOpen(true)} />
          <IconButton size="sm" icon="phone-hang-up" label="End call" onClick={() => endVoice()} />
        </div>
      </Match>
      <Match when={!voice.view && voice.notice}>
        <div class="x-voice-pill" style={{ left: centre() }} role="alert">
          <Icon name="microphone-off" size={14} />
          <span class="x-voice-pill-label x-voice-notice">{voice.notice}</span>
          <Button size="sm" variant="ghost" onClick={() => dismissVoiceNotice()}>Dismiss</Button>
        </div>
      </Match>
    </Switch>
  );
}
