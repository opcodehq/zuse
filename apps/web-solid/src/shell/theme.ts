// Applies the theme preference, following the system setting when asked to.
import { view } from "@/core/view";
import { createEffect, createRoot, createSignal } from "solid-js";

const media = window.matchMedia("(prefers-color-scheme: dark)");
const [systemDark, setSystemDark] = createSignal(media.matches);
media.addEventListener("change", (event) => setSystemDark(event.matches));

export const resolvedTheme = () => {
  const theme = view.preferences.theme;
  return theme === "system" ? (systemDark() ? "dark" : "light") : theme;
};

createRoot(() =>
  createEffect(resolvedTheme, (theme) => {
    document.documentElement.dataset.theme = theme;
    document.querySelector('meta[name="theme-color"]')?.setAttribute("content", theme === "dark" ? "#151515" : "#ffffff");
  }),
);
