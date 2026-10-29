//! Exact grants and chat overrides. Evaluation and durable audit happen on the host.
use crate::{EngineError, new_id, now_ms};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use zuse_protocol::{SandboxLevel, permission::*};

type Result<T> = std::result::Result<T, EngineError>;
fn error(e: impl std::fmt::Display) -> EngineError {
    EngineError::Other(format!("permission rules: {e}"))
}

#[derive(Clone)]
pub struct PermissionRules {
    path: PathBuf,
}
impl PermissionRules {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().into(),
        }
    }
    fn db(&self) -> Result<Connection> {
        let db = Connection::open(&self.path).map_err(error)?;
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(error)?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS permission_rules (
            id TEXT PRIMARY KEY, project TEXT NOT NULL, chat TEXT NOT NULL,
            tool TEXT NOT NULL, input TEXT NOT NULL, effect TEXT NOT NULL,
            created INTEGER NOT NULL, updated INTEGER NOT NULL,
            UNIQUE(project,chat,tool,input));
            CREATE TABLE IF NOT EXISTS permission_audit (
            seq INTEGER PRIMARY KEY, project TEXT NOT NULL, at INTEGER NOT NULL,
            chat TEXT NOT NULL, tool TEXT NOT NULL, decision TEXT NOT NULL,
            source TEXT NOT NULL, rule_id TEXT, details TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS permission_audit_project ON permission_audit(project,seq DESC);",
        )
        .map_err(error)?;
        Ok(db)
    }
    pub fn snapshot(&self, project: &str, chat: &str) -> Result<PermissionSnapshot> {
        let db = self.db()?;
        let mut statement = db.prepare("SELECT id,project,chat,tool,input,effect,created,updated FROM permission_rules WHERE project=? AND (chat='' OR chat=?) ORDER BY updated DESC,id").map_err(error)?;
        let rules = statement
            .query_map(params![project, chat], |r| {
                let chat: String = r.get(2)?;
                let input: String = r.get(4)?;
                let effect: String = r.get(5)?;
                Ok(PermissionRule {
                    id: r.get(0)?,
                    project: r.get(1)?,
                    chat_id: (!chat.is_empty()).then_some(chat),
                    tool: r.get(3)?,
                    input: serde_json::from_str(&input).unwrap_or_default(),
                    effect: match effect.as_str() {
                        "allow" => RuleEffect::Allow,
                        "deny" => RuleEffect::Deny,
                        _ => RuleEffect::Ask,
                    },
                    created_at: r.get(6)?,
                    updated_at: r.get(7)?,
                })
            })
            .map_err(error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(error)?;
        let mut statement = db.prepare("SELECT at,chat,tool,decision,source,rule_id,details FROM permission_audit WHERE project=? ORDER BY seq DESC LIMIT 100").map_err(error)?;
        let activity = statement
            .query_map([project], |r| {
                Ok(PermissionAudit {
                    at: r.get(0)?,
                    chat_id: r.get(1)?,
                    tool: r.get(2)?,
                    decision: r.get(3)?,
                    source: r.get(4)?,
                    rule_id: r.get(5)?,
                    details: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
                })
            })
            .map_err(error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(error)?;
        Ok(PermissionSnapshot {
            project: project.into(),
            rules,
            activity,
        })
    }
    pub fn save(&self, project: &str, chat: Option<&str>, approval: &ToolApproval) -> Result<()> {
        if forced(approval) {
            return Err(error("This action requires approval each time"));
        }
        let mut db = self.db()?;
        let tx = db.transaction().map_err(error)?;
        let id = new_id();
        let at = now_ms();
        tx.execute("INSERT INTO permission_rules(id,project,chat,tool,input,effect,created,updated) VALUES (?,?,?,?,?,'allow',?,?) ON CONFLICT(project,chat,tool,input) DO UPDATE SET effect='allow',updated=excluded.updated", params![id,project,chat.unwrap_or(""),rule_tool(approval),input_key(&rule_input(approval)),at,at]).map_err(error)?;
        let rule_id: String = tx
            .query_row(
                "SELECT id FROM permission_rules WHERE project=? AND chat=? AND tool=? AND input=?",
                params![
                    project,
                    chat.unwrap_or(""),
                    rule_tool(approval),
                    input_key(&rule_input(approval))
                ],
                |r| r.get(0),
            )
            .map_err(error)?;
        audit(
            &tx,
            project,
            chat.unwrap_or(""),
            &approval.tool,
            "allow",
            "rule saved",
            Some(&rule_id),
            serde_json::json!({"chatScope":chat,"inputHash":input_hash(&approval.input)}),
        )?;
        tx.commit().map_err(error)
    }
    pub fn edit(
        &self,
        project: &str,
        owner_chat: &str,
        id: &str,
        effect: RuleEffect,
        chat_only: bool,
    ) -> Result<()> {
        let mut db = self.db()?;
        let tx = db.transaction().map_err(error)?;
        let tool:String=tx.query_row("SELECT tool FROM permission_rules WHERE project=? AND id=? AND (chat='' OR chat=?)",params![project,id,owner_chat],|r|r.get(0)).map_err(error)?;
        tx.execute(
            "UPDATE permission_rules SET effect=?,chat=?,updated=? WHERE id=?",
            params![
                match effect {
                    RuleEffect::Allow => "allow",
                    RuleEffect::Ask => "ask",
                    RuleEffect::Deny => "deny",
                },
                if chat_only { owner_chat } else { "" },
                now_ms(),
                id
            ],
        )
        .map_err(error)?;
        audit(
            &tx,
            project,
            owner_chat,
            &tool,
            &format!("{effect:?}"),
            "rule edited",
            Some(id),
            serde_json::json!({"chatScope":if chat_only {Some(owner_chat)}else{None},"effect":effect}),
        )?;
        tx.commit().map_err(error)
    }
    pub fn remove(&self, project: &str, chat: &str, id: &str) -> Result<()> {
        let mut db = self.db()?;
        let tx = db.transaction().map_err(error)?;
        let tool:String=tx.query_row("SELECT tool FROM permission_rules WHERE project=? AND id=? AND (chat='' OR chat=?)",params![project,id,chat],|r|r.get(0)).map_err(error)?;
        tx.execute("DELETE FROM permission_rules WHERE id=?", [id])
            .map_err(error)?;
        audit(
            &tx,
            project,
            chat,
            &tool,
            "removed",
            "rule removed",
            Some(id),
            serde_json::json!({"requestedBy":chat}),
        )?;
        tx.commit().map_err(error)
    }
    /// None means ask, including the native automatic reviewer's escalations.
    pub fn decide(
        &self,
        project: &str,
        chat: &str,
        mode: SandboxLevel,
        approval: &ToolApproval,
    ) -> Result<Option<bool>> {
        let db = self.db()?;
        let (decision, source, rule) = if mode == SandboxLevel::ReadOnly {
            (Some(false), "read-only ceiling", None)
        } else if mode.automatic_review() {
            (None, "native reviewer escalation", None)
        } else if forced(approval) {
            (None, "forced prompt", None)
        } else {
            let matched: Option<(String,String)> = db.query_row(
                "SELECT id,effect FROM permission_rules WHERE project=? AND (chat='' OR chat=?) AND tool=? AND input=? ORDER BY (chat<>'') DESC LIMIT 1",
                params![project,chat,rule_tool(approval),input_key(&rule_input(approval))],
                |r|Ok((r.get(0)?,r.get(1)?)),
            ).optional().map_err(error)?;
            match matched {
                Some((id, effect)) => (
                    match effect.as_str() {
                        "allow" => Some(true),
                        "deny" => Some(false),
                        _ => None,
                    },
                    "saved rule",
                    Some(id),
                ),
                None => (None, "approval required", None),
            }
        };
        audit(
            &db,
            project,
            chat,
            &approval.tool,
            match decision {
                Some(true) => "allow",
                Some(false) => "deny",
                None => "ask",
            },
            source,
            rule.as_deref(),
            serde_json::json!({"sandbox":mode,"inputHash":input_hash(&approval.input)}),
        )?;
        Ok(decision)
    }
    pub fn record_answer(
        &self,
        project: &str,
        chat: &str,
        approval: &ToolApproval,
        allowed: bool,
        mode: SandboxLevel,
    ) -> Result<()> {
        audit(
            &self.db()?,
            project,
            chat,
            &approval.tool,
            if allowed { "allow" } else { "deny" },
            "user answer",
            None,
            serde_json::json!({"sandbox":mode,"inputHash":input_hash(&approval.input)}),
        )
    }
}
fn approval_payload(input: &serde_json::Value) -> &serde_json::Value {
    if input.get("harness").is_some() && input.get("cwd").is_some() && input.get("input").is_some()
    {
        &input["input"]
    } else {
        input
    }
}
fn file_body_fields(approval: &ToolApproval) -> Option<&'static [&'static str]> {
    let (path, fields): (&str, &'static [&'static str]) = match approval.tool.as_str() {
        "claude:Write" | "claude:Edit" | "claude:MultiEdit" => (
            "file_path",
            &[
                "content",
                "old_string",
                "new_string",
                "replace_all",
                "edits",
            ],
        ),
        "zuse:write_file" | "zuse:edit_file" | "zuse:multi_edit" => (
            "path",
            &[
                "content",
                "old_string",
                "new_string",
                "replace_all",
                "edits",
                "expected_hash",
            ],
        ),
        "pi:write" | "pi:edit" => ("path", &["content", "oldText", "newText"]),
        _ => return None,
    };
    approval_payload(&approval.input)
        .get(path)
        .and_then(serde_json::Value::as_str)?;
    Some(fields)
}
fn rule_tool(approval: &ToolApproval) -> &str {
    if file_body_fields(approval).is_some() {
        match approval.tool.split(':').next() {
            Some("claude") => "claude:file-write",
            Some("zuse") => "zuse:file-write",
            Some("pi") => "pi:file-write",
            _ => &approval.tool,
        }
    } else {
        &approval.tool
    }
}
/// Known schemas grant a file target or command, while retaining all other context.
/// Unclassified tools keep their complete input, including ACP tool kind.
fn rule_input(approval: &ToolApproval) -> serde_json::Value {
    let mut input = approval.input.clone();
    let wrapped = input.get("harness").is_some()
        && input.get("cwd").is_some()
        && input.get("input").is_some();
    let payload = if wrapped {
        input.get_mut("input").unwrap()
    } else {
        &mut input
    };
    let fields = file_body_fields(approval).or_else(|| {
        let command = payload.get("command")?;
        if !(command.is_string()
            || command.as_array().is_some_and(|parts| {
                !parts.is_empty() && parts.iter().all(serde_json::Value::is_string)
            }))
        {
            return None;
        }
        match approval.tool.as_str() {
            "claude:Bash" | "pi:bash" => Some(&["description", "timeout", "timeout_ms"]),
            "codex:item/commandExecution/requestApproval" => Some(&["reason"]),
            _ => None,
        }
    });
    if let (Some(fields), Some(payload)) = (fields, payload.as_object_mut()) {
        for field in fields {
            payload.remove(*field);
        }
    }
    input
}
fn declared_escalation(input: &serde_json::Value) -> bool {
    match input {
        serde_json::Value::Object(fields) => fields.iter().any(|(key, value)| match key.as_str() {
            "dangerouslyDisableSandbox" | "dangerously_disable_sandbox" if value == true => true,
            "sandbox_permissions" | "sandboxPermissions" if value == "require_escalated" => true,
            "additionalPermissions" | "grantRoot" | "networkApprovalContext"
                if !value.is_null() =>
            {
                true
            }
            "kind" if value == "switch_mode" || value == "switchMode" => true,
            _ => declared_escalation(value),
        }),
        serde_json::Value::Array(values) => values.iter().any(declared_escalation),
        _ => false,
    }
}

fn input_key(input: &serde_json::Value) -> String {
    fn canonical(input: &serde_json::Value) -> serde_json::Value {
        match input {
            serde_json::Value::Object(fields) => {
                let mut keys: Vec<_> = fields.keys().collect();
                keys.sort();
                serde_json::Value::Object(
                    keys.into_iter()
                        .map(|key| (key.clone(), canonical(&fields[key])))
                        .collect(),
                )
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(canonical).collect())
            }
            value => value.clone(),
        }
    }
    canonical(input).to_string()
}
fn input_hash(input: &serde_json::Value) -> String {
    format!("{:x}", Sha256::digest(input_key(input).as_bytes()))
}

fn audit(
    db: &Connection,
    project: &str,
    chat: &str,
    tool: &str,
    decision: &str,
    source: &str,
    rule: Option<&str>,
    details: serde_json::Value,
) -> Result<()> {
    db.execute("INSERT INTO permission_audit(project,at,chat,tool,decision,source,rule_id,details) VALUES (?,?,?,?,?,?,?,?)",params![project,now_ms(),chat,tool,decision,source,rule,details.to_string()]).map_err(error)?;
    Ok(())
}
/// Persistent grants cannot silence credential, private, or plan-exit prompts.
pub fn forced(approval: &ToolApproval) -> bool {
    if approval.force_prompt || declared_escalation(&approval.input) {
        return true;
    }
    let value = format!("{} {}", approval.tool, rule_input(approval)).to_ascii_lowercase();
    if approval.tool.to_ascii_lowercase().contains("login")
        || (file_body_fields(approval).is_none() && value.contains("login"))
    {
        return true;
    }
    [
        ".env",
        ".ssh",
        ".aws",
        ".gnupg",
        ".netrc",
        ".pgpass",
        "id_ecdsa",
        "id_dsa",
        ".p12",
        ".pfx",
        "credentials",
        "private-secret",
        "exitplanmode",
        "exit_plan",
        "id_rsa",
        "id_ed25519",
        ".pem",
        ".key",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

impl crate::sessions::SessionsEngine {
    pub fn permission_rules(&self) -> PermissionRules {
        PermissionRules::new(self.permission_rules_path())
    }
    pub(crate) fn prepare_permission_input(
        &self,
        chat: &str,
        run: &str,
        questions: &mut Vec<zuse_protocol::UserInputQuestion>,
    ) -> Vec<zuse_protocol::UserInputAnswer> {
        let context = self.permission_context(chat, Some(run));
        let mut answers = Vec::new();
        questions.retain_mut(|question| {
            let Some(approval) = question.approval.as_mut() else {
                return true;
            };
            let Some((project, cwd, mode)) = &context else {
                answers.push(zuse_protocol::UserInputAnswer {
                    question_id: question.id.clone(),
                    labels: vec![approval.deny_label.clone()],
                });
                return false;
            };
            let Some(harness) = self.permission_harness(chat, run) else {
                answers.push(zuse_protocol::UserInputAnswer { question_id: question.id.clone(), labels: vec![approval.deny_label.clone()] });
                return false;
            };
            approval.input = serde_json::json!({"harness":harness,"cwd":cwd,"input":approval.input});
            // Provider labels must not impersonate the host's persistence choices.
            if question.options.iter().any(|label| label == ALLOW_CHAT || label == ALLOW_PROJECT) {
                approval.force_prompt = true;
            }
            // The native reviewer already declined to decide this escalation.
            if mode.automatic_review() {
                approval.force_prompt = true;
            }
            // Persist through this host, using one-time choices on the provider wire.
            question.options.retain(|label| label == &approval.allow_label || label == &approval.deny_label);
            let decision = self
                .permission_rules()
                .decide(project, chat, *mode, approval);
            match decision {
                Ok(Some(allowed)) => {
                    let allowed = allowed && self.permission_context(chat, Some(run)).is_some_and(|(_, _, current)| current == *mode);
                    answers.push(zuse_protocol::UserInputAnswer {
                        question_id: question.id.clone(),
                        labels: vec![if allowed {
                            approval.allow_label.clone()
                        } else {
                            approval.deny_label.clone()
                        }],
                    });
                    false
                }
                Ok(None) => {
                    if !forced(approval)
                        && !question.multi_select
                        && question.options.contains(&approval.allow_label)
                        && question.options.contains(&approval.deny_label)
                    {
                        question
                            .options
                            .extend([ALLOW_CHAT.into(), ALLOW_PROJECT.into()]);
                    }
                    true
                }
                Err(err) => {
                    // Without a working audit store, never grant an unaudited action.
                    tracing::error!(%err,"Permission evaluation failed; declining action");
                    self.permission_failure(chat,run,"Could not read or audit approval rules. The action was declined. Open Saved permission rules to retry.");
                    answers.push(zuse_protocol::UserInputAnswer {
                        question_id: question.id.clone(),
                        labels: vec![approval.deny_label.clone()],
                    });
                    false
                }
            }
        });
        answers
    }
    pub(crate) fn finish_permission_input(
        &self,
        chat: &str,
        run: &str,
        questions: &[zuse_protocol::UserInputQuestion],
        answers: Vec<zuse_protocol::UserInputAnswer>,
    ) -> Vec<zuse_protocol::UserInputAnswer> {
        let mut normalized = Vec::with_capacity(questions.len());
        for question in questions {
            let Some(approval) = &question.approval else {
                normalized.extend(
                    answers
                        .iter()
                        .filter(|answer| answer.question_id == question.id)
                        .cloned(),
                );
                continue;
            };
            let mut matching = answers
                .iter()
                .filter(|answer| answer.question_id == question.id);
            let labels = matching
                .next()
                .map(|answer| answer.labels.as_slice())
                .unwrap_or(&[]);
            let unambiguous = matching.next().is_none();
            let mut allowed = false;
            if let Some((project, _, mode)) = self.permission_context(chat, Some(run)) {
                if mode != SandboxLevel::ReadOnly && unambiguous && labels.len() == 1 {
                    let label = &labels[0];
                    if label == &approval.allow_label {
                        allowed = true;
                    } else if label == &approval.deny_label {
                        allowed = false;
                    } else if question.options.contains(label)
                        && !forced(approval)
                        && (label == ALLOW_CHAT || label == ALLOW_PROJECT)
                    {
                        let scope = (label == ALLOW_CHAT).then_some(chat);
                        match self.permission_rules().save(&project, scope, approval) {
                            Ok(()) => allowed = true,
                            Err(err) => {
                                tracing::error!(%err, "Could not save approval rule; declining action");
                                self.permission_failure(chat, run,
                                    "Could not save the permission rule. The action was declined. Open Saved permission rules to retry.");
                            }
                        }
                    }
                }
                if let Err(err) = self
                    .permission_rules()
                    .record_answer(&project, chat, approval, allowed, mode)
                {
                    tracing::error!(%err, "Could not audit approval; declining action");
                    self.permission_failure(
                        chat,
                        run,
                        "Could not record the approval decision. The action was declined.",
                    );
                    allowed = false;
                }
                allowed &= self
                    .permission_context(chat, Some(run))
                    .is_some_and(|(_, _, current)| current == mode);
            }
            normalized.push(zuse_protocol::UserInputAnswer {
                question_id: question.id.clone(),
                labels: vec![if allowed {
                    approval.allow_label.clone()
                } else {
                    approval.deny_label.clone()
                }],
            });
        }
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn approval() -> ToolApproval {
        ToolApproval::new(
            "test:bash",
            serde_json::json!({"cwd":"/project","command":"cargo test"}),
            "Yes",
            "No",
        )
    }
    fn store() -> (tempfile::TempDir, PermissionRules) {
        let dir = tempfile::tempdir().unwrap();
        let rules = PermissionRules::new(dir.path().join("rules.sqlite"));
        (dir, rules)
    }
    #[test]
    fn file_grants_bind_paths_across_edits_without_persisting_contents() {
        let (_dir, rules) = store();
        let first = ToolApproval::new(
            "claude:Write",
            serde_json::json!({"harness":"claude-code", "cwd":"/project", "input":{"file_path":"src/login.rs", "content":"login credentials implementation"}}),
            "Yes",
            "No",
        );
        rules.save("p", None, &first).unwrap();
        let mut next = first.clone();
        next.input["input"]["content"] = serde_json::json!("different implementation");
        assert_eq!(
            rules
                .decide("p", "c", SandboxLevel::WorkspaceWrite, &next)
                .unwrap(),
            Some(true)
        );
        assert_eq!(
            rules
                .decide("p", "c", SandboxLevel::ReadOnly, &next)
                .unwrap(),
            Some(false)
        );
        let snapshot = rules.snapshot("p", "c").unwrap();
        assert_eq!(snapshot.rules[0].tool, "claude:file-write");
        assert!(snapshot.rules[0].input["input"].get("content").is_none());
        next.tool = "claude:Edit".into();
        next.input["input"] = serde_json::json!({"file_path":"src/login.rs", "old_string":"old", "new_string":"new", "replace_all":false});
        assert_eq!(
            rules
                .decide("p", "c", SandboxLevel::WorkspaceWrite, &next)
                .unwrap(),
            Some(true)
        );
        next.input["input"]["file_path"] = serde_json::json!("src/other.rs");
        assert_eq!(
            rules
                .decide("p", "c", SandboxLevel::WorkspaceWrite, &next)
                .unwrap(),
            None
        );
        next.input["input"]["file_path"] = serde_json::json!(".env");
        assert!(rules.save("p", None, &next).is_err());
    }
    #[test]
    fn command_grants_ignore_display_hints_and_keep_execution_context() {
        let (_dir, rules) = store();
        let first = ToolApproval::new(
            "claude:Bash",
            serde_json::json!({"command":"cargo test", "cwd":"/project", "description":"Run tests", "timeout":1000}),
            "Yes",
            "No",
        );
        rules.save("p", None, &first).unwrap();
        let mut next = first.clone();
        next.input["description"] = serde_json::json!("Verify tests");
        next.input["timeout"] = serde_json::json!(5000);
        assert_eq!(
            rules
                .decide("p", "c", SandboxLevel::WorkspaceWrite, &next)
                .unwrap(),
            Some(true)
        );
        next.input["cwd"] = serde_json::json!("/other");
        assert_eq!(
            rules
                .decide("p", "c", SandboxLevel::WorkspaceWrite, &next)
                .unwrap(),
            None
        );
        next.input["cwd"] = serde_json::json!("/project");
        next.input["run_in_background"] = serde_json::json!(true);
        assert_eq!(
            rules
                .decide("p", "c", SandboxLevel::WorkspaceWrite, &next)
                .unwrap(),
            None
        );
    }
    #[test]
    fn unknown_file_schemas_keep_complete_input_matching() {
        let (_dir, rules) = store();
        let first = ToolApproval::new(
            "custom:write",
            serde_json::json!({"path":"file.rs", "content":"one"}),
            "Yes",
            "No",
        );
        rules.save("p", None, &first).unwrap();
        let mut next = first.clone();
        next.input["content"] = serde_json::json!("two");
        assert_eq!(
            rules
                .decide("p", "c", SandboxLevel::WorkspaceWrite, &next)
                .unwrap(),
            None
        );
    }
    #[test]
    fn declared_escalations_and_mode_switches_cannot_be_saved() {
        let (_dir, rules) = store();
        for input in [
            serde_json::json!({"command":"cargo test", "dangerouslyDisableSandbox":true}),
            serde_json::json!({"kind":"switch_mode", "rawInput":{"mode":"full-access"}}),
        ] {
            let approval = ToolApproval::new("test:tool", input, "Yes", "No");
            assert!(rules.save("p", None, &approval).is_err());
            assert_eq!(
                rules
                    .decide("p", "c", SandboxLevel::WorkspaceWrite, &approval)
                    .unwrap(),
                None
            );
            assert_eq!(
                rules
                    .decide("p", "c", SandboxLevel::ReadOnly, &approval)
                    .unwrap(),
                Some(false)
            );
        }
    }

    #[tokio::test]
    async fn unanswered_content_questions_keep_the_original_cancellation_shape() {
        let dir = tempfile::tempdir().unwrap();
        let engine = crate::SessionsEngine::new(
            "test".into(),
            std::sync::Arc::new(crate::RunJournal::open(dir.path()).unwrap()),
            std::sync::Arc::new(crate::HarnessRegistry::new()),
        );
        let question = zuse_protocol::UserInputQuestion {
            approval: None,
            id: "content".into(),
            header: "Question".into(),
            question: "Continue?".into(),
            options: vec!["Yes".into(), "No".into()],
            multi_select: false,
            prefill: None,
            multiline: false,
        };
        assert!(
            engine
                .finish_permission_input("chat", "run", std::slice::from_ref(&question), vec![])
                .is_empty()
        );
        let answer = zuse_protocol::UserInputAnswer {
            question_id: question.id.clone(),
            labels: vec!["Yes".into()],
        };
        assert_eq!(
            engine.finish_permission_input("chat", "run", &[question], vec![answer.clone()]),
            vec![answer]
        );
    }

    #[test]
    fn exact_matching_never_treats_commands_as_prefixes_or_patterns() {
        let (_dir, rules) = store();
        let a = approval();
        rules.save("p", None, &a).unwrap();
        assert_eq!(
            rules
                .decide("p", "c", SandboxLevel::WorkspaceWrite, &a)
                .unwrap(),
            Some(true)
        );
        for input in [
            serde_json::json!({"cwd":"/project","command":"cargo test && destructive"}),
            serde_json::json!({"cwd":"/elsewhere","command":"cargo test"}),
            serde_json::json!({"cwd":"/project","command":"cargo *"}),
        ] {
            let mut other = a.clone();
            other.input = input;
            assert_eq!(
                rules
                    .decide("p", "c", SandboxLevel::WorkspaceWrite, &other)
                    .unwrap(),
                None
            );
        }
        let mut other = a;
        other.tool = "other:bash".into();
        assert_eq!(
            rules
                .decide("p", "c", SandboxLevel::WorkspaceWrite, &other)
                .unwrap(),
            None
        );
    }
    #[test]
    fn scopes_are_isolated_and_chat_overrides_project() {
        let (_dir, rules) = store();
        let a = approval();
        rules.save("p", None, &a).unwrap();
        rules.save("p", Some("c"), &a).unwrap();
        let chat = rules
            .snapshot("p", "c")
            .unwrap()
            .rules
            .into_iter()
            .find(|r| r.chat_id.is_some())
            .unwrap();
        for (effect, expected) in [
            (RuleEffect::Deny, Some(false)),
            (RuleEffect::Ask, None),
            (RuleEffect::Allow, Some(true)),
        ] {
            rules.edit("p", "c", &chat.id, effect, true).unwrap();
            assert_eq!(
                rules
                    .decide("p", "c", SandboxLevel::WorkspaceWrite, &a)
                    .unwrap(),
                expected
            );
            assert_eq!(
                rules
                    .decide("p", "other", SandboxLevel::WorkspaceWrite, &a)
                    .unwrap(),
                Some(true)
            );
            assert_eq!(
                rules
                    .decide("other", "c", SandboxLevel::WorkspaceWrite, &a)
                    .unwrap(),
                None
            );
        }
        rules.remove("p", "c", &chat.id).unwrap();
        assert_eq!(
            rules
                .decide("p", "c", SandboxLevel::WorkspaceWrite, &a)
                .unwrap(),
            Some(true)
        );
    }
    #[test]
    fn persistence_edits_revocation_and_audit_survive_reopen() {
        let (dir, rules) = store();
        let a = approval();
        rules.save("p", Some("c"), &a).unwrap();
        let id = rules.snapshot("p", "c").unwrap().rules[0].id.clone();
        drop(rules);
        let rules = PermissionRules::new(dir.path().join("rules.sqlite"));
        assert_eq!(
            rules
                .decide("p", "c", SandboxLevel::WorkspaceWrite, &a)
                .unwrap(),
            Some(true)
        );
        rules.edit("p", "c", &id, RuleEffect::Deny, false).unwrap();
        assert_eq!(
            rules
                .decide("p", "other", SandboxLevel::WorkspaceWrite, &a)
                .unwrap(),
            Some(false)
        );
        rules.remove("p", "c", &id).unwrap();
        let snapshot = rules.snapshot("p", "c").unwrap();
        assert!(snapshot.rules.is_empty());
        assert!(
            snapshot
                .activity
                .iter()
                .any(|e| e.source == "rule removed" && e.rule_id.as_deref() == Some(&id))
        );
        assert!(snapshot.activity.iter().any(|e| e.source == "rule edited"));
        assert!(
            snapshot
                .activity
                .iter()
                .any(|e| e.source == "saved rule" && e.rule_id.as_deref() == Some(&id))
        );
        assert_eq!(
            rules
                .decide("p", "c", SandboxLevel::WorkspaceWrite, &a)
                .unwrap(),
            None
        );
    }
    #[test]
    fn read_only_ceiling_beats_every_grant() {
        let (_dir, rules) = store();
        let a = approval();
        rules.save("p", None, &a).unwrap();
        rules.save("p", Some("c"), &a).unwrap();
        assert_eq!(
            rules.decide("p", "c", SandboxLevel::ReadOnly, &a).unwrap(),
            Some(false)
        );
        assert_eq!(
            rules.snapshot("p", "c").unwrap().activity[0].source,
            "read-only ceiling"
        );
    }
    #[test]
    fn forced_and_sensitive_prompts_cannot_be_saved_or_silenced() {
        let (_dir, rules) = store();
        let mut a = approval();
        rules.save("p", None, &a).unwrap();
        a.force_prompt = true;
        assert_eq!(
            rules
                .decide("p", "c", SandboxLevel::WorkspaceWrite, &a)
                .unwrap(),
            None
        );
        assert!(rules.save("p", None, &a).is_err());
        a.force_prompt = false;
        a.input = serde_json::json!({"path":"/project/.env.local"});
        assert!(rules.save("p", Some("c"), &a).is_err());
        assert_eq!(
            rules.decide("p", "c", SandboxLevel::ReadOnly, &a).unwrap(),
            Some(false)
        );
    }
    #[test]
    fn foreign_project_and_chat_cannot_edit_or_remove_rules() {
        let (_dir, rules) = store();
        rules.save("p", Some("c"), &approval()).unwrap();
        let id = rules.snapshot("p", "c").unwrap().rules[0].id.clone();
        assert!(rules.remove("other", "c", &id).is_err());
        assert!(rules.remove("p", "other", &id).is_err());
        assert!(
            rules
                .edit("p", "other", &id, RuleEffect::Allow, false)
                .is_err()
        );
        assert_eq!(rules.snapshot("p", "c").unwrap().rules.len(), 1);
    }
    #[test]
    fn repeated_grants_upsert_and_scope_conflicts_leave_prior_rules_intact() {
        let (_dir, rules) = store();
        let a = approval();
        rules.save("p", None, &a).unwrap();
        rules.save("p", None, &a).unwrap();
        assert_eq!(rules.snapshot("p", "c").unwrap().rules.len(), 1);
        rules.save("p", Some("c"), &a).unwrap();
        let id = rules
            .snapshot("p", "c")
            .unwrap()
            .rules
            .into_iter()
            .find(|r| r.chat_id.is_some())
            .unwrap()
            .id;
        assert!(rules.edit("p", "c", &id, RuleEffect::Deny, false).is_err());
        assert_eq!(rules.snapshot("p", "c").unwrap().rules.len(), 2);
    }
    #[test]
    fn native_automatic_review_escalations_still_ask() {
        let (_dir, rules) = store();
        rules.save("p", None, &approval()).unwrap();
        assert_eq!(
            rules
                .decide("p", "c", SandboxLevel::Auto, &approval())
                .unwrap(),
            None
        );
        assert_eq!(
            rules.snapshot("p", "c").unwrap().activity[0].source,
            "native reviewer escalation"
        );
    }
    #[test]
    fn unauditable_decisions_fail_closed_and_mutations_roll_back() {
        let dir = tempfile::tempdir().unwrap();
        let rules = PermissionRules::new(dir.path().join("rules.sqlite"));
        let approval = ToolApproval::new(
            "test:exec",
            serde_json::json!({"command":"cargo test"}),
            "Yes",
            "No",
        );
        rules.save("p", None, &approval).unwrap();
        let id = rules.snapshot("p", "c").unwrap().rules[0].id.clone();
        rules.db().unwrap().execute_batch("CREATE TRIGGER fail_audit BEFORE INSERT ON permission_audit BEGIN SELECT RAISE(ABORT,'audit unavailable'); END;").unwrap();
        assert!(
            rules
                .decide("p", "c", SandboxLevel::WorkspaceWrite, &approval)
                .is_err()
        );
        assert!(rules.edit("p", "c", &id, RuleEffect::Deny, false).is_err());
        assert!(rules.remove("p", "c", &id).is_err());
        let mut other = approval.clone();
        other.input = serde_json::json!({"command":"cargo check"});
        assert!(rules.save("p", None, &other).is_err());
        let snapshot = rules.snapshot("p", "c").unwrap();
        assert_eq!(snapshot.rules.len(), 1);
        assert_eq!(snapshot.rules[0].effect, RuleEffect::Allow);
    }
    #[test]
    fn structured_object_order_is_canonical_for_matching_and_audit_digests() {
        let value: serde_json::Value =
            serde_json::from_str(r#"{"z":{"b":2,"a":1},"a":[{"z":2,"a":1}]}"#).unwrap();
        assert_eq!(
            input_key(&value),
            r#"{"a":[{"a":1,"z":2}],"z":{"a":1,"b":2}}"#
        );
        assert_eq!(input_hash(&value).len(), 64);
    }
}
