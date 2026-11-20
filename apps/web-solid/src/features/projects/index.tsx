// Projects area: New project, the project archive, and project-scoped controls other areas mount.
import "./projects.css";

export { NewProject } from "./NewProject";
export { ProjectArchive } from "./ProjectArchive";

// For other areas: the session info card mounts ProjectActions, the composer tray mounts
// ProjectCreationNotice, project lists use CreationBadge. Worktree location lives in Settings → General.
export { actionTerminals,claimActionTerminal,type ActionTerminal } from "./actions";
export { CreationBadge,ProjectCreationNotice } from "./CreationNotice";
export { creationBlocks,creationForSpace } from "./creations";
export { ActionEditor,ProjectActions } from "./ProjectActions";
export { openNewProject } from "./setup";
