//! The coordinator's reference: what used to ride in every brief (action
//! shapes, lifecycle, the failure playbook, issue writing, reporting) as
//! guides it opens when it acts in that area. Every coordinator reads them
//! with `zapper_files {zapperId, path}`; on Claude Code they are also project
//! skills in the Zapper's workspace. Action shapes come from the command
//! schema, so the guide can't drift from what the engine accepts.
use super::*;

pub(super) struct Guide {
    /// Read-only path under [`GUIDE_DIR`], as `zapper_files` serves it.
    pub path: &'static str,
    /// Its Claude Code skill name.
    pub skill: &'static str,
    /// When to open it: the skill description and the brief's index line.
    pub when: &'static str,
    pub body: String,
}

/// Paths under this prefix are the engine's guides, not Zapper files.
pub(super) const GUIDE_DIR: &str = "guide/";

pub(super) fn guides() -> Vec<Guide> {
    vec![
        Guide {
            path: "guide/commands.md",
            skill: "zapper-commands",
            when: "every zapper_command action with its fields, delegating and picking models, events and acknowledging",
            body: commands(),
        },
        Guide {
            path: "guide/lifecycle.md",
            skill: "zapper-lifecycle",
            when: "issue statuses, owners and what each move needs, asking the user",
            body: LIFECYCLE.to_owned(),
        },
        Guide {
            path: "guide/failures.md",
            skill: "zapper-failures",
            when: "what to do when a command fails: conflicts, refusals, bad params",
            body: FAILURES.to_owned(),
        },
        Guide {
            path: "guide/issues.md",
            skill: "zapper-issues",
            when: "writing issues, splitting requests and briefing threads",
            body: ISSUES.to_owned(),
        },
        Guide {
            path: "guide/reporting.md",
            skill: "zapper-reporting",
            when: "issue comments and replies to the user",
            body: REPORTING.to_owned(),
        },
    ]
}

/// One guide as a read-only Zapper file.
pub(super) fn file(path: &str) -> Option<ZapperFile> {
    guides()
        .into_iter()
        .find(|g| g.path == path)
        .map(|g| ZapperFile {
            path: g.path.to_owned(),
            version: 0,
            content: g.body,
            deleted: false,
        })
}

/// The brief's index of guides.
pub(super) fn index() -> String {
    guides()
        .iter()
        .map(|g| format!("- {} ({}): {}.\n", g.path, g.skill, g.when))
        .collect()
}

/// Writes the guides as Claude Code project skills under `workspace`,
/// touching only files whose content changed.
pub(super) fn install_skills(workspace: &Path) -> Result<()> {
    for guide in guides() {
        let dir = workspace.join(".claude/skills").join(guide.skill);
        let content = format!(
            "---\nname: {}\ndescription: Zapper reference: {}. Open before acting in that area.\n---\n\n{}",
            guide.skill, guide.when, guide.body
        );
        let path = dir.join("SKILL.md");
        if std::fs::read_to_string(&path).ok().as_deref() != Some(content.as_str()) {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&path, content)?;
        }
    }
    Ok(())
}

fn commands() -> String {
    format!(
        r#"# Zapper commands

zapper_command makes every change: {{"commandId": a new unique id per command (reuse it only to retry a command whose result was lost), "zapperId", "expectedRevision": the revision you read, "action": {{"type": ..., fields}}}}. Its tool schema is the source of truth for shapes.

Actions (? = optional): {catalog}.

## Fields
- details (createIssue, editIssue) = {{title, description, doneCriteria, outOfScope, status: "todo" or "backlog", priority, owner: {{"type": "zapper"}}, assigned: true, parent: issue id or null, dueDate: null or "YYYY-MM-DD", labels: [], relations: [issue ids]}}.
- editIssue: details replaces the whole record, so send every field. If the board marks the issue descriptionClipped, read it with zapper_issue first. Owners: {{"type": "zapper"}} or {{"type": "thread", "chatId": ...}}; hand an issue to the user only with askUser.
- delegate {{issueId, worker: {{chatId: "zapper-worker-<commandId>", deviceId, spaceId, objective, followups: 0}}, model: {{harness, model}}, base: a ref such as "origin/main"}}. Pick deviceId, spaceId and model with list_projects, list_harnesses and list_models. Use a cheaper model for mechanical work. Run issues in parallel only when they touch separate parts of the code; otherwise base the next one on the previous branch. Beyond maxConcurrentWorkers running threads, a delegated thread is queued: its worker shows queuePosition (1 starts next) and its issue queued, and it starts by itself when a running thread finishes. Don't re-delegate it.
- linkIssuePullRequest url: "https://github.com/owner/repo/pull/N".
- messageWorker {{chatId, text, automatic: true}}.
- writeFile expectedVersion: the file's current version from zapper_files. Seeded files start at version 1; a new file is 0. Writing INSTRUCTIONS.md also sets the purpose the app shows, from its first paragraph. Paths under guide/ are read-only.
- restoreFile {{path, version, expectedVersion}}: brings an earlier version back as the newest one.
- decide {{decision: {{id, workerChatId: chat id or null, question, answer: null}}}}: only for a question that belongs to no issue; prefer askUser.
- Status values: backlog, todo, inProgress, needsYou, blocked, inReview, done, canceled. Priority: urgent, high, normal, low.

## Results and reads
zapper_command returns a compact result (new revision, the changed issue, worker, decision or file, and a summary); add "fullState": true to a command only if you need the whole state back.
zapper_issue {{zapperId}} reads the whole board; zapper_issue {{zapperId, issueId}} reads one issue in full (issueId or its code, such as ZAP-3).
zapper_files {{zapperId, history?}} reads your files with versions; zapper_files {{zapperId, path}} reads one, including these guides.
zapper_events {{zapperId}} lists thread reports and blockers; zapper_acknowledge {{zapperId, eventIds}} once handled. Several updates can arrive in one message, each with its event id.
For issueChanged events, inspect the issue, do what it needs, then acknowledge. Don't comment just to acknowledge.
"#,
        catalog = zuse_protocol::zappers::action_catalog()
    )
}

const LIFECYCLE: &str = r#"# Issue lifecycle

- Every open issue has exactly one owner: you (zapper), a thread, or the user. Create the issue and start its thread in the same turn: createIssue with owner zapper, then delegate with its issueId, which makes the thread the owner and moves the issue to inProgress. Start as soon as the next step is clear; don't wait for perfect detail. Hold an issue only when a missing decision would change the architecture or the doneCriteria.
- Lifecycle, enforced by the engine: todo → inProgress (a thread owns it) → inReview (a pull request is linked; ready means checks green, no blocking review findings, mergeable, and behaviour matches the request) → closeIssue merged (after the merge) or closeIssue dropped (with the reason). Blocked means you own it and are unblocking it.
- When only the user can unblock an issue, askUser on it: one question, with your recommendation. That makes them the owner and the issue Needs you. Their reply hands it back automatically; record their exact decision with a comment and continue in the same turn. Never invent an answer, and don't leave a Needs-you issue stale: remind them in your reply.
- A person's comment whose `delivery` on the board names a thread is that thread's: `queued` or `retrying` is still on its way, `sent` means the thread's queue took it, and its reply comes back as that thread's report. Don't relay it with messageWorker. When `delivery` is `held` or `cancelled`, the thread never got it: `delivery.detail` says why, and the comment is yours to handle; so is one whose `delivery` names you.
- Threads you stopped, or that stopped or failed by themselves, you can resume with messageWorker. A thread the user stopped needs their word first.
"#;

const FAILURES: &str = r#"# When a command fails

- Revision conflict: what your action depends on (the issue or decision it changes, or the whole Zapper for status and config changes) changed since you read it. Re-read what the error names, then retry once with the revision it gives and a new commandId. Comments, new issues, links, messages and file writes don't conflict on the revision.
- File version conflict: re-read the file with zapper_files, merge, and write with its current version.
- "needs explicit user continuation" on messageWorker: the user stopped that thread. Don't retry; askUser on its issue whether to continue it. Threads you stopped, or that stopped or failed by themselves, you can resume with messageWorker.
- Bad params: compare your action with the shapes in guide/commands.md and the zapper_command schema, and fix the fields. Don't loop on guesses.
- A lifecycle refusal ("link the pull request before…", "an issue is inProgress only while a thread works on it…", "close issues with closeIssue…") names the missing evidence: provide it, don't work around it.
"#;

const ISSUES: &str = r#"# Writing issues and briefing threads

- Issues are how work is tracked, not chat. Use one for anything that outlives this turn, involves a thread, or waits on the user; skip it only for an answer you finish right now. A request with several separately shippable changes gets one parent issue for the request and one child issue per change (details.parent).
- Every issue has a goal (description), doneCriteria and outOfScope. No issue without doneCriteria.
- Brief each thread through its issue and objective so it can finish without asking: goal, doneCriteria, outOfScope, what you already know (files, commands, prior findings) and how to verify. Threads end with a report that starts "Status: done" or "Status: blocked".
- Ambiguity: state an assumption and keep going, unless it changes architecture or doneCriteria. Log any scope change on the issue as a comment; never widen scope silently or rewrite broadly when a bounded fix was asked.
- WORKERS.md is prepended to every thread's prompt, so keep it to standing rules for threads (conventions, how to verify) and never use it as a log.
"#;

const REPORTING: &str = r#"# Comments and replies

- Comment on the issue at each meaningful change: PR opened, CI failed or recovered, review findings, decision asked or received, merge or drop.
- When you reply to the user, lead with what needs them, then deltas since last time: done, in progress, blocked, with issue codes and PR links. Don't repeat logs, narrate tool calls, or hedge ("should work", "probably fine"); say exactly what is unverified. When work completes, propose the one highest-leverage next step.
- DIGEST.md is a short status summary for the user; keep it current when the picture changes.
"#;
