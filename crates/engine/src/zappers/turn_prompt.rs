//! What each coordinator turn carries. The brief (role, hard rules, an index
//! of guides, purpose) reaches a coordinator conversation once: as the
//! harness's system prompt where the harness takes one, otherwise as the
//! conversation's first turn. No turn carries the full state: the first one
//! summarizes the board in a few lines, later ones carry the message plus
//! what changed since the previous turn, and the coordinator reads more with
//! zapper_issue. The brief rides again only when it changes, and a fresh
//! conversation (a new coordinator chat) starts over. What was sent is kept
//! per coordinator chat, so a restart never loses it.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use zuse_protocol::HarnessId;

/// Zapper files whose versions turns report, so the coordinator knows when
/// to read one again. INSTRUCTIONS.md is the purpose doc.
const CONTEXT_FILES: [&str; 3] = ["INSTRUCTIONS.md", "NOTES.md", "DIGEST.md"];

/// Largest brief sent as a launch argument. A longer one (a very long
/// purpose) rides as a turn instead, so the process always starts.
const SYSTEM_PROMPT_MAX: usize = 24 * 1024;

/// Most lines a turn lists before pointing at zapper_issue for the rest.
const MAX_LINES: usize = 12;

/// Harnesses whose adapter takes a system prompt from the engine. Claude Code
/// launches with it as `--append-system-prompt`; every other harness gets the
/// brief as a turn.
fn takes_system_prompt(harness: HarnessId) -> bool {
    matches!(harness, HarnessId::ClaudeCode)
}

/// What a coordinator chat has been sent: the harness, how and which brief,
/// and the board as of its last turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Ledger {
    harness: HarnessId,
    /// `system:<sha256>` or `turn:<sha256>` of the brief.
    brief: String,
    board: Board,
}

/// The parts of the state a turn's delta reports.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Board {
    revision: u32,
    issues: BTreeMap<String, IssueLine>,
    /// Worker chat id → its hold reason, if stopped.
    workers: BTreeMap<String, Option<String>>,
    /// Open decision id → question.
    decisions: BTreeMap<String, String>,
    /// Context file path → version.
    files: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct IssueLine {
    code: String,
    title: String,
    status: String,
    owner: String,
}

impl IssueLine {
    fn render(&self, id: &str, was: Option<&str>) -> String {
        let was = was.map(|w| format!(" (was {w})")).unwrap_or_default();
        format!(
            "{} {}{was}, owner {}: {} (issueId {id})",
            self.code, self.status, self.owner, self.title
        )
    }
}

impl Board {
    fn of(zot: &Zot, holds: &BTreeMap<String, String>, files: &[ZotFile]) -> Self {
        let text = |v: serde_json::Value| v.as_str().unwrap_or_default().to_owned();
        Self {
            revision: zot.revision,
            issues: zot
                .issues
                .iter()
                .map(|i| {
                    let owner = match &i.details.owner {
                        None => "none".to_owned(),
                        Some(ZotIssueOwner::Zapper) => "zapper".to_owned(),
                        Some(ZotIssueOwner::User) => "user".to_owned(),
                        Some(ZotIssueOwner::Thread { chat_id }) => format!("thread {chat_id}"),
                    };
                    let line = IssueLine {
                        code: i.code(),
                        title: i.details.title.clone(),
                        status: text(serde_json::to_value(i.details.status).unwrap_or_default()),
                        owner,
                    };
                    (i.id.clone(), line)
                })
                .collect(),
            workers: zot
                .workers
                .iter()
                .map(|w| (w.chat_id.clone(), holds.get(&w.chat_id).cloned()))
                .collect(),
            decisions: zot
                .decisions
                .iter()
                .filter(|d| d.answer.is_none())
                .map(|d| (d.id.clone(), d.question.clone()))
                .collect(),
            files: files.iter().map(|f| (f.path.clone(), f.version)).collect(),
        }
    }

    /// One line per change from `before` to `self`.
    fn changes(&self, before: &Board) -> Vec<String> {
        let mut lines = Vec::new();
        for (id, issue) in &self.issues {
            match before.issues.get(id) {
                None => lines.push(format!("new: {}", issue.render(id, None))),
                Some(old) if old != issue => lines
                    .push(issue.render(id, (old.status != issue.status).then_some(&*old.status))),
                Some(_) => {}
            }
        }
        for (id, issue) in &before.issues {
            if !self.issues.contains_key(id) {
                lines.push(format!("{} removed", issue.code));
            }
        }
        let reason = |r: &str| r.split_once(": ").map_or(r, |(_, r)| r).to_lowercase();
        for (chat, hold) in &self.workers {
            match (before.workers.get(chat), hold) {
                (None, None) => lines.push(format!("Thread {chat} started")),
                (None, Some(r)) => lines.push(format!("Thread {chat} added, {}", reason(r))),
                (Some(None), Some(r)) => lines.push(format!("Thread {chat} {}", reason(r))),
                (Some(Some(_)), None) => lines.push(format!("Thread {chat} resumed")),
                _ => {}
            }
        }
        for chat in before.workers.keys() {
            if !self.workers.contains_key(chat) {
                lines.push(format!("Thread {chat} released"));
            }
        }
        for (id, question) in &self.decisions {
            if !before.decisions.contains_key(id) {
                lines.push(format!("Decision {id} open: {question}"));
            }
        }
        for id in before.decisions.keys() {
            if !self.decisions.contains_key(id) {
                lines.push(format!("Decision {id} answered"));
            }
        }
        for (path, version) in &self.files {
            if before.files.get(path) != Some(version) {
                lines.push(format!(
                    "{path} changed (version {version}); read it with zapper_files"
                ));
            }
        }
        lines
    }
}

/// A composed coordinator turn.
#[derive(Debug)]
pub(super) struct Turn {
    pub prompt: String,
    /// The brief, when the harness takes it as its system prompt.
    pub system_prompt: Option<String>,
    pub ledger: Ledger,
}

/// Up to [`MAX_LINES`] lines as a list, then a pointer for the rest.
fn list(lines: Vec<String>) -> String {
    let more = lines.len().saturating_sub(MAX_LINES);
    let mut out: String = lines
        .into_iter()
        .take(MAX_LINES)
        .map(|l| format!("- {l}\n"))
        .collect();
    if more > 0 {
        out.push_str(&format!("- {more} more: zapper_issue {{zotId}}\n"));
    }
    out
}

/// The first turn's few lines about the board: open issues and decisions,
/// counts and file versions. The rest is a zapper_issue read away.
fn summary(zot: &Zot, board: &Board) -> String {
    let open: Vec<_> = zot
        .issues
        .iter()
        .filter(|i| !i.details.status.terminal())
        .collect();
    let mut lines: Vec<String> = open
        .iter()
        .filter_map(|i| board.issues.get(&i.id).map(|l| l.render(&i.id, None)))
        .collect();
    lines.extend(
        board
            .decisions
            .iter()
            .map(|(id, q)| format!("Decision {id} open: {q}")),
    );
    let stopped = board.workers.values().filter(|h| h.is_some()).count();
    let files = board
        .files
        .iter()
        .map(|(p, v)| format!("{p} v{v}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "## Board (revision {})\n{}{} open issues, {} closed; {} threads ({stopped} stopped).\nFiles: {files}.",
        zot.revision,
        list(lines),
        open.len(),
        zot.issues.len() - open.len(),
        board.workers.len(),
    )
}

/// The brief as it stands, purpose doc included.
fn brief(zot: &Zot, files: &[ZotFile]) -> String {
    let soul = files.iter().find(|f| f.path == PURPOSE_FILE);
    coordinator_brief(zot, &purpose_section(zot, soul))
}

/// Builds the turn for `message`.
pub(super) fn compose(
    zot: &Zot,
    harness: HarnessId,
    previous: Option<&Ledger>,
    holds: &BTreeMap<String, String>,
    files: &[ZotFile],
    message: &str,
) -> Turn {
    let brief = brief(zot, files);
    let hook = takes_system_prompt(harness) && brief.len() <= SYSTEM_PROMPT_MAX;
    let key = format!(
        "{}:{:x}",
        if hook { "system" } else { "turn" },
        Sha256::digest(brief.as_bytes())
    );
    let board = Board::of(zot, holds, files);
    let mut prompt = String::new();
    match previous.filter(|p| p.harness == harness) {
        None => {
            if !hook {
                prompt.push_str(&brief);
                prompt.push_str("\n\n");
            }
            prompt.push_str(&summary(zot, &board));
        }
        Some(previous) => {
            if previous.brief != key {
                if hook {
                    prompt.push_str(
                        "Your brief changed; your system prompt now carries the current version.\n\n",
                    );
                } else {
                    prompt.push_str(&brief);
                    prompt.push_str("\n\n");
                }
            }
            let changes = board.changes(&previous.board);
            if changes.is_empty() {
                prompt.push_str(&format!(
                    "## Since your last turn\nNo changes (revision {}).",
                    zot.revision
                ));
            } else {
                prompt.push_str(&format!(
                    "## Since your last turn (revision {}, was {})\n{}Details: zapper_issue {{zotId, issueId}}.",
                    zot.revision,
                    previous.board.revision,
                    list(changes)
                ));
            }
        }
    }
    prompt.push_str("\n\n## User message\n");
    prompt.push_str(message);
    Turn {
        prompt,
        system_prompt: hook.then_some(brief),
        ledger: Ledger {
            harness,
            brief: key,
            board,
        },
    }
}

impl Zots {
    /// Wraps a coordinator's message as its turn and stamps the system
    /// prompt on harnesses that take one.
    pub(super) fn prepare_coordinator_turn(
        &self,
        zot: &Zot,
        chat: &str,
        harness: HarnessId,
        request: &mut zuse_protocol::RunRequest,
    ) -> Result<()> {
        let files: Vec<_> = self
            .files(&zot.id, false)?
            .into_iter()
            .filter(|f| CONTEXT_FILES.contains(&f.path.as_str()))
            .collect();
        let (holds, previous) = {
            let db = self
                .inner
                .db
                .lock()
                .map_err(|_| anyhow::anyhow!("Zapper storage unavailable"))?;
            let holds = db
                .prepare("SELECT chat,reason FROM zot_worker_holds WHERE zot=?1")?
                .query_map([&zot.id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<BTreeMap<String, String>>>()?;
            let previous: Option<String> = db
                .query_row(
                    "SELECT ledger FROM zot_coordinator_turns WHERE chat=?1",
                    [chat],
                    |r| r.get(0),
                )
                .optional()?;
            // An unreadable ledger only means the next turn starts over.
            (holds, previous.and_then(|l| serde_json::from_str(&l).ok()))
        };
        let turn = compose(
            zot,
            harness,
            previous.as_ref(),
            &holds,
            &files,
            &request.prompt,
        );
        if turn.system_prompt.is_some() {
            guide::install_skills(&self.workspace(&zot.id)?)?;
        }
        request.prompt = turn.prompt;
        let option = zuse_providers::claude::SYSTEM_PROMPT_OPTION;
        match turn.system_prompt {
            Some(brief) => {
                request.model_options.insert(option.into(), brief.into());
            }
            None => {
                request.model_options.remove(option);
            }
        }
        self.inner
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Zapper storage unavailable"))?
            .execute(
                "INSERT OR REPLACE INTO zot_coordinator_turns(chat,zot,ledger) VALUES(?1,?2,?3)",
                params![chat, zot.id, serde_json::to_string(&turn.ledger)?],
            )?;
        Ok(())
    }

    /// Forgets what a coordinator chat was sent, after a turn that never
    /// reached it, so its next turn carries the brief and full state again.
    pub fn forget_coordinator_turn(&self, chat: &str) -> Result<()> {
        self.inner
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Zapper storage unavailable"))?
            .execute("DELETE FROM zot_coordinator_turns WHERE chat=?1", [chat])?;
        Ok(())
    }

    /// One Zapper file, or one of the engine's read-only guides.
    pub fn file(&self, id: &str, path: &str) -> Result<ZotFile> {
        if path.starts_with(guide::GUIDE_DIR) {
            self.get(id)?;
            return guide::file(path).with_context(|| format!("no guide at {path}"));
        }
        self.files(id, false)?
            .into_iter()
            .find(|f| f.path == path)
            .with_context(|| format!("no Zapper file at {path}"))
    }

    /// The whole board: every issue, thread and decision.
    pub fn coordinator_state(&self, id: &str) -> Result<serde_json::Value> {
        issues::coordinator_view(&self.get(id)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zots::tests::{command, create, worker};

    fn request(prompt: &str) -> zuse_protocol::RunRequest {
        serde_json::from_value(serde_json::json!({
            "prompt": prompt, "cwd": "/tmp", "sandbox": "workspace-write"
        }))
        .unwrap()
    }

    fn turn(store: &Zots, harness: HarnessId, text: &str) -> zuse_protocol::RunRequest {
        let zot = store.get("release").unwrap();
        let mut request = request(text);
        store
            .prepare_request(&zot.coordinator_chat_id, harness, &mut request)
            .unwrap();
        request
    }

    fn system_prompt(request: &zuse_protocol::RunRequest) -> Option<&str> {
        request
            .model_options
            .get(zuse_providers::claude::SYSTEM_PROMPT_OPTION)
            .and_then(|v| v.as_str())
    }

    #[test]
    fn claude_gets_the_brief_as_its_system_prompt_and_later_turns_only_the_delta() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let first = turn(&store, HarnessId::ClaudeCode, "Plan the release");
        let brief = system_prompt(&first).unwrap();
        assert!(brief.contains("## Hard rules"));
        assert!(brief.contains("guide/commands.md (zapper-commands)"));
        assert!(brief.contains("## Purpose (INSTRUCTIONS.md, version 1)\nCoordinate the release"));
        assert!(!first.prompt.contains("## Hard rules"));
        assert!(first.prompt.starts_with("## Board (revision"));
        assert!(first.prompt.contains("NOTES.md v1"));
        assert!(!first.prompt.contains("\"issues\""), "no state dump");
        assert!(first.prompt.ends_with("## User message\nPlan the release"));
        let skill = store
            .workspace("release")
            .unwrap()
            .join(".claude/skills/zapper-commands/SKILL.md");
        let skill = std::fs::read_to_string(skill).unwrap();
        assert!(skill.starts_with("---\nname: zapper-commands\n"));
        assert!(skill.contains(&zuse_protocol::zots::action_catalog()));

        let second = turn(&store, HarnessId::ClaudeCode, "Anything new?");
        assert_eq!(system_prompt(&second), Some(brief));
        assert!(!second.prompt.contains("## Board (revision"));
        assert!(!second.prompt.contains("## Hard rules"));
        assert!(
            second
                .prompt
                .starts_with("## Since your last turn\nNo changes")
        );
        assert!(second.prompt.ends_with("## User message\nAnything new?"));
        assert!(second.prompt.len() < 200, "{}", second.prompt);
    }

    #[test]
    fn harnesses_without_a_system_prompt_get_the_brief_once_as_the_first_turn() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let first = turn(&store, HarnessId::Codex, "Plan the release");
        assert_eq!(system_prompt(&first), None);
        assert!(first.prompt.starts_with("You are Release, a Zapper"));
        assert!(first.prompt.contains("## Board (revision"));
        assert!(!store.workspace("release").unwrap().join(".claude").exists());
        let second = turn(&store, HarnessId::Codex, "Anything new?");
        assert!(!second.prompt.contains("You are Release"));
        assert!(second.prompt.starts_with("## Since your last turn"));

        // Switching harness starts the conversation's context over.
        let switched = turn(&store, HarnessId::ClaudeCode, "Still there?");
        assert!(system_prompt(&switched).is_some());
        assert!(switched.prompt.starts_with("## Board (revision"));
    }

    #[test]
    fn later_turns_list_what_changed_since_the_last_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        turn(&store, HarnessId::ClaudeCode, "Start");
        store
            .command(command(
                "release",
                "adopt",
                ZotAction::AdoptWorker { worker: worker() },
            ))
            .unwrap();
        let zot = store.get("release").unwrap();
        let notes = store
            .files("release", false)
            .unwrap()
            .into_iter()
            .find(|f| f.path == "NOTES.md")
            .unwrap();
        store
            .command(command(
                "release",
                "notes",
                ZotAction::WriteFile {
                    path: "NOTES.md".into(),
                    content: "plan".into(),
                    expected_version: notes.version,
                },
            ))
            .unwrap();
        let next = turn(&store, HarnessId::ClaudeCode, "Status?");
        let revision = store.get("release").unwrap().revision;
        assert!(
            next.prompt.starts_with(&format!(
                "## Since your last turn (revision {revision}, was {})",
                zot.revision - 1
            )),
            "{}",
            next.prompt
        );
        assert!(next.prompt.contains("- Thread worker started"));
        assert!(next.prompt.contains(&format!(
            "- NOTES.md changed (version {}); read it with zapper_files",
            notes.version + 1
        )));
        assert!(
            next.prompt
                .contains("Details: zapper_issue {zotId, issueId}.")
        );

        store
            .command(command(
                "release",
                "stop",
                ZotAction::StopWorker {
                    chat_id: "worker".into(),
                },
            ))
            .unwrap();
        let stopped = turn(&store, HarnessId::ClaudeCode, "And now?");
        assert!(
            stopped
                .prompt
                .contains("- Thread worker stopped by the user"),
            "{}",
            stopped.prompt
        );
    }

    #[test]
    fn a_changed_brief_is_sent_again() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let mut config = store.get("release").unwrap().config;
        let first = turn(&store, HarnessId::Codex, "Start");
        let claude_dir = tempfile::tempdir().unwrap();
        let claude = Zots::open(claude_dir.path(), "local").unwrap();
        create(&claude, "release");
        let claude_first = turn(&claude, HarnessId::ClaudeCode, "Start");
        config.purpose = "Ship the new installer".into();
        for store in [&store, &claude] {
            store
                .command(command(
                    "release",
                    "purpose",
                    ZotAction::Configure {
                        config: config.clone(),
                    },
                ))
                .unwrap();
        }
        let resent = turn(&store, HarnessId::Codex, "Go");
        assert!(resent.prompt.starts_with("You are Release, a Zapper"));
        assert!(resent.prompt.contains("Ship the new installer"));
        assert!(resent.prompt.contains("## Since your last turn"));
        assert!(!resent.prompt.contains("## Board (revision"));
        assert!(!first.prompt.contains("Ship the new installer"));
        let again = turn(&store, HarnessId::Codex, "Go on");
        assert!(!again.prompt.contains("You are Release"));

        let updated = turn(&claude, HarnessId::ClaudeCode, "Go");
        assert!(
            system_prompt(&updated)
                .unwrap()
                .contains("Ship the new installer")
        );
        assert_ne!(system_prompt(&updated), system_prompt(&claude_first));
        assert!(updated.prompt.starts_with("Your brief changed"));
        assert!(!updated.prompt.contains("You are Release"));
    }

    #[test]
    fn a_fresh_conversation_or_a_lost_turn_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        turn(&store, HarnessId::Codex, "Start");
        assert!(
            turn(&store, HarnessId::Codex, "More")
                .prompt
                .starts_with("## Since")
        );
        store
            .command(command("release", "fresh", ZotAction::FreshConversation))
            .unwrap();
        let fresh = turn(&store, HarnessId::Codex, "Hello again");
        assert!(fresh.prompt.starts_with("You are Release, a Zapper"));
        assert!(fresh.prompt.contains("## Board (revision"));

        let chat = store.get("release").unwrap().coordinator_chat_id;
        store.forget_coordinator_turn(&chat).unwrap();
        let retried = turn(&store, HarnessId::Codex, "Hello again");
        assert!(retried.prompt.starts_with("You are Release, a Zapper"));
    }

    #[test]
    fn the_brief_and_later_turns_stay_small() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        for n in 0..30 {
            store
                .command(command(
                    "release",
                    &format!("issue-{n}"),
                    ZotAction::CreateIssue {
                        issue_id: format!("issue-{n}"),
                        details: ZotIssueDetails {
                            title: format!("Issue {n}"),
                            description: "A long description. ".repeat(100),
                            done_criteria: "Tests pass.".into(),
                            assigned: true,
                            owner: Some(ZotIssueOwner::Zapper),
                            ..Default::default()
                        },
                    },
                ))
                .unwrap();
        }
        let zot = store.get("release").unwrap();
        let files = store.files("release", false).unwrap();
        let soul = files.iter().find(|f| f.path == PURPOSE_FILE);
        let purpose = purpose_section(&zot, soul).len();
        let brief = brief(&zot, &files);
        assert!(brief.len() - purpose < 3 * 1024, "{}", brief.len());
        let first = turn(&store, HarnessId::ClaudeCode, "Start");
        assert!(first.prompt.contains("- 18 more: zapper_issue {zotId}"));
        assert!(first.prompt.len() < 2 * 1024, "{}", first.prompt.len());
        // Every issue changes at once: the turn still lists only a few.
        for n in 0..30 {
            let mut details = store.get("release").unwrap().issues[n].details.clone();
            details.title = format!("Issue {n}, renamed");
            store
                .command(command(
                    "release",
                    &format!("rename-{n}"),
                    ZotAction::EditIssue {
                        issue_id: format!("issue-{n}"),
                        details,
                    },
                ))
                .unwrap();
        }
        let later = turn(&store, HarnessId::ClaudeCode, "");
        assert!(later.prompt.len() < 1024, "{}", later.prompt.len());
    }

    #[test]
    fn guides_are_read_only_files_built_from_the_command_schema() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let commands = store.file("release", "guide/commands.md").unwrap();
        assert!(
            commands
                .content
                .contains(&zuse_protocol::zots::action_catalog())
        );
        for guide in guide::guides() {
            let files = store.files("release", false).unwrap();
            assert!(brief(&store.get("release").unwrap(), &files).contains(guide.path));
            assert!(store.file("release", guide.path).is_ok());
        }
        assert!(store.file("release", "guide/missing.md").is_err());
        assert_eq!(store.file("release", "NOTES.md").unwrap().version, 1);
        let refused = store
            .command(command(
                "release",
                "overwrite-guide",
                ZotAction::WriteFile {
                    path: "guide/commands.md".into(),
                    content: "x".into(),
                    expected_version: 0,
                },
            ))
            .unwrap_err();
        assert!(refused.to_string().contains("read-only guide"), "{refused}");
    }
}
