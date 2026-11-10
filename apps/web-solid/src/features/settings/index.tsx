// Settings and usage. Importing this area also starts its app-wide effects: session sounds, browser
// notifications, agent update notices and reduced motion.
import "./settings.css";
import "./alerts";
import { lazy } from "solid-js";

export { Settings } from "./Settings";
export const Usage = lazy(() => import("./usage/Usage").then((m) => ({ default: m.Usage })));
