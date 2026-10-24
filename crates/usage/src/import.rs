//! Source importers inspect usage metadata without launching a provider or writing its files.
use crate::{Data, Record, SourceStatus, TokenCounts};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Claude,
    Codex,
    OpenCode,
    Amp,
    Pi,
    Grok,
    Native,
    Legacy,
}
#[derive(Debug, Clone)]
pub struct Source {
    pub id: String,
    pub label: String,
    pub format: Format,
    /// Roots are explicit so isolated profiles and tests never scan another user's history.
    pub roots: Vec<PathBuf>,
}
impl Source {
    pub fn new(id: &str, label: &str, format: Format, roots: Vec<PathBuf>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            format,
            roots,
        }
    }
}
#[derive(Clone)]
struct Parsed {
    modified: Option<SystemTime>,
    size: u64,
    format: Format,
    records: Vec<Record>,
    skipped: usize,
}
#[derive(Default)]
pub struct Importer {
    files: HashMap<PathBuf, Parsed>,
}
impl Importer {
    pub fn invalidate(&mut self) {
        self.files.clear();
    }
    pub fn scan(&mut self, sources: &[Source]) -> Data {
        let mut result = Data::default();
        let mut visited = std::collections::HashSet::new();
        for source in sources {
            let mut files = Vec::new();
            for root in &source.roots {
                collect(root, source.format, &mut files);
            }
            files.sort();
            files.dedup();
            let detected = source.roots.iter().any(|p| p.exists());
            let mut records = Vec::new();
            let mut skipped = 0;
            let mut failures = 0;
            for file in &files {
                visited.insert(file.clone());
                match self.read(file, source) {
                    Ok(parsed) => {
                        records.extend(parsed.records);
                        skipped += parsed.skipped;
                    }
                    Err(_) => failures += 1,
                }
            }
            // Claude exports can repeat the same message with a larger completed count.
            let mut identities: BTreeMap<String, Record> = BTreeMap::new();
            for record in records {
                match identities.get(&record.id) {
                    Some(previous)
                        if source.format != Format::Claude
                            || previous.tokens.total() >= record.tokens.total() => {}
                    _ => {
                        identities.insert(record.id.clone(), record);
                    }
                }
            }
            let records: Vec<_> = identities.into_values().collect();
            let warning = if failures > 0 || skipped > 0 {
                Some(format!(
                    "{failures} usage files could not be read; {skipped} usage rows lacked valid counts or timestamps."
                ))
            } else if source.format == Format::Grok {
                Some(
                    "Grok records finished subagent tokens only; main-agent usage is unavailable."
                        .into(),
                )
            } else if source.format == Format::Native {
                Some("Native journals record input/output totals; historical cache counts and prices may be unavailable.".into())
            } else if !detected {
                Some("No local usage source was found.".into())
            } else if records.is_empty() {
                Some("Detected, but no usage rows were found.".into())
            } else {
                None
            };
            result.sources.push(SourceStatus {
                id: source.id.clone(),
                label: source.label.clone(),
                detected,
                record_count: records.len(),
                paths: source
                    .roots
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect(),
                warning,
            });
            result.records.extend(records);
        }
        self.files.retain(|path, _| visited.contains(path));
        result.deduplicate();
        let mut merged: BTreeMap<String, SourceStatus> = BTreeMap::new();
        for status in result.sources.drain(..) {
            match merged.get_mut(&status.id) {
                None => {
                    merged.insert(status.id.clone(), status);
                }
                Some(previous) => {
                    if status.detected && !previous.detected {
                        previous.warning = status.warning.clone();
                    } else if status.detected && previous.detected && status.warning.is_some() {
                        previous.warning = Some(
                            [previous.warning.take(), status.warning]
                                .into_iter()
                                .flatten()
                                .collect::<Vec<_>>()
                                .join(" "),
                        );
                    }
                    previous.detected |= status.detected;
                    previous.paths.extend(status.paths);
                    previous.paths.sort();
                    previous.paths.dedup();
                }
            }
        }
        result.sources = merged
            .into_values()
            .map(|mut status| {
                status.record_count = result
                    .records
                    .iter()
                    .filter(|record| record.source_id == status.id)
                    .count();
                status
            })
            .collect();
        result
    }
    fn read(&mut self, path: &Path, source: &Source) -> Result<Parsed, Box<dyn std::error::Error>> {
        let metadata = fs::metadata(path)?;
        let modified = metadata.modified().ok();
        // SQLite may change through its WAL without changing the database's metadata.
        let database = matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("db" | "sqlite")
        );
        if !database && let Some(cached) = self.files.get(path) {
            if cached.modified == modified
                && cached.size == metadata.len()
                && cached.format == source.format
            {
                return Ok(cached.clone());
            }
        }
        let mut parsed = Parsed {
            modified,
            size: metadata.len(),
            format: source.format,
            records: Vec::new(),
            skipped: 0,
        };
        if database {
            read_database(path, source, &mut parsed)?;
        } else if matches!(source.format, Format::Amp | Format::OpenCode) {
            let value: Value = serde_json::from_reader(BufReader::new(fs::File::open(path)?))?;
            if source.format == Format::Amp {
                read_amp(&value, path, source, &mut parsed);
            } else if let Some(row) = open_code(&value, path, source) {
                parsed.records.push(row);
            }
        } else {
            let lines = BufReader::new(fs::File::open(path)?)
                .lines()
                .map_while(Result::ok)
                .filter_map(|line| serde_json::from_str::<Value>(&line).ok());
            match source.format {
                Format::Claude => read_claude(lines, path, source, &mut parsed),
                Format::Codex => read_codex(lines, path, source, &mut parsed),
                Format::Pi => read_pi(lines, path, source, &mut parsed),
                Format::Grok => read_grok(lines, path, source, &mut parsed),
                Format::Native => read_native(lines, path, source, &mut parsed),
                _ => {}
            }
        }
        if !database {
            self.files.insert(path.to_owned(), parsed.clone());
        }
        Ok(parsed)
    }
}
fn collect(root: &Path, format: Format, files: &mut Vec<PathBuf>) {
    if root.is_file() {
        if accepted(root, format) {
            files.push(root.to_owned());
        }
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect(&entry.path(), format, files);
        } else if kind.is_file() && accepted(&entry.path(), format) {
            files.push(entry.path());
        }
    }
}
fn accepted(path: &Path, format: Format) -> bool {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    match format {
        Format::OpenCode => name.ends_with(".json") || name == "opencode.db",
        Format::Amp => name.ends_with(".json"),
        Format::Legacy => name.ends_with(".sqlite"),
        Format::Grok => name == "updates.jsonl",
        _ => name.ends_with(".jsonl"),
    }
}
fn text(v: &Value) -> Option<String> {
    v.as_str()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
}
fn number(v: &Value) -> u64 {
    if let Some(text) = v.as_str() {
        return text.parse::<u64>().unwrap_or(0);
    }
    v.as_u64()
        .or_else(|| {
            v.as_f64()
                .filter(|n| n.is_finite() && *n >= 0.)
                .map(|n| n as u64)
        })
        .unwrap_or(0)
}
fn timestamp(v: &Value) -> Option<DateTime<Utc>> {
    if let Some(s) = v.as_str() {
        return DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc));
    }
    let n = v.as_i64()?;
    DateTime::from_timestamp_millis(if n.abs() < 10_000_000_000 {
        n.checked_mul(1000)?
    } else {
        n
    })
}
fn record(source: &Source, path: &Path, time: DateTime<Utc>, tokens: TokenCounts) -> Record {
    Record {
        id: String::new(),
        source_id: source.id.clone(),
        source_label: source.label.clone(),
        provider_id: source.id.clone(),
        model: "unknown".into(),
        session_id: None,
        project_path: None,
        workspace_path: None,
        started_at: time,
        ended_at: time,
        tokens,
        cost_usd: None,
        provenance: path.to_string_lossy().into_owned(),
        confidence: "exact".into(),
        possible_duplicate: false,
    }
}
fn file_session(path: &Path) -> Option<String> {
    path.file_stem().map(|s| s.to_string_lossy().into_owned())
}
fn read_claude(lines: impl Iterator<Item = Value>, path: &Path, source: &Source, out: &mut Parsed) {
    for v in lines {
        let usage = &v["message"]["usage"];
        if !usage.is_object() || v["isApiErrorMessage"] == true {
            continue;
        }
        let Some(time) = timestamp(&v["timestamp"]) else {
            out.skipped += 1;
            continue;
        };
        let tokens = TokenCounts {
            input_tokens: number(&usage["input_tokens"]),
            output_tokens: number(&usage["output_tokens"]),
            cache_read_tokens: number(&usage["cache_read_input_tokens"]),
            cache_creation_tokens: number(&usage["cache_creation_input_tokens"]),
            reasoning_tokens: 0,
        };
        if tokens.total() == 0 {
            continue;
        }
        let mut row = record(source, path, time, tokens);
        row.model = text(&v["message"]["model"]).unwrap_or(row.model);
        row.session_id = text(&v["sessionId"]).or_else(|| file_session(path));
        row.project_path = text(&v["cwd"]);
        row.workspace_path = row.project_path.clone();
        row.cost_usd = v["costUSD"].as_f64();
        let identity = text(&v["message"]["id"])
            .map(|id| format!("{id}:{}", v["requestId"].as_str().unwrap_or_default()))
            .unwrap_or_else(|| {
                format!(
                    "{}:{time}:{}",
                    path.display(),
                    serde_json::to_string(&row.tokens).unwrap()
                )
            });
        row.identify(&identity);
        out.records.push(row);
    }
}
#[derive(Clone, Copy, Default)]
struct CodexCount {
    input: u64,
    cached: u64,
    output: u64,
    reasoning: u64,
    total: u64,
}
impl CodexCount {
    fn parse(v: &Value) -> Option<Self> {
        v.is_object().then(|| Self {
            input: number(&v["input_tokens"]),
            cached: number(
                v.get("cached_input_tokens")
                    .unwrap_or(&v["cache_read_input_tokens"]),
            ),
            output: number(&v["output_tokens"]),
            reasoning: number(&v["reasoning_output_tokens"]),
            total: number(&v["total_tokens"]),
        })
    }
    fn subtract(self, b: Self) -> Self {
        Self {
            input: self.input.saturating_sub(b.input),
            cached: self.cached.saturating_sub(b.cached),
            output: self.output.saturating_sub(b.output),
            reasoning: self.reasoning.saturating_sub(b.reasoning),
            total: self.total.saturating_sub(b.total),
        }
    }
}
fn read_codex(lines: impl Iterator<Item = Value>, path: &Path, source: &Source, out: &mut Parsed) {
    let mut model = "gpt-5".to_owned();
    let mut cwd = None;
    let mut previous = CodexCount::default();
    let session = source
        .roots
        .iter()
        .find_map(|root| path.strip_prefix(root).ok())
        .map(|p| p.with_extension("").to_string_lossy().into_owned())
        .or_else(|| file_session(path));
    for v in lines {
        let p = &v["payload"];
        if v["type"] == "session_meta" || v["type"] == "turn_context" {
            cwd = text(&p["cwd"]).or(cwd);
        }
        let found = text(&p["info"]["model"])
            .or_else(|| text(&p["info"]["model_name"]))
            .or_else(|| text(&p["info"]["metadata"]["model"]))
            .or_else(|| text(&p["model"]))
            .or_else(|| text(&p["metadata"]["model"]));
        if let Some(found) = found {
            model = found;
        }
        if v["type"] != "event_msg" || p["type"] != "token_count" {
            continue;
        }
        let info = &p["info"];
        let totals = CodexCount::parse(&info["total_token_usage"]);
        let count = CodexCount::parse(&info["last_token_usage"])
            .or_else(|| totals.map(|n| n.subtract(previous)));
        if let Some(totals) = totals {
            previous = totals;
        }
        let Some(count) = count else {
            continue;
        };
        let cached = count.cached.min(count.input);
        let tokens = TokenCounts {
            input_tokens: count.input - cached,
            output_tokens: count.output,
            cache_read_tokens: cached,
            cache_creation_tokens: 0,
            reasoning_tokens: count.reasoning,
        };
        if tokens.total() == 0 {
            continue;
        }
        let Some(time) = timestamp(&v["timestamp"]) else {
            out.skipped += 1;
            continue;
        };
        let mut row = record(source, path, time, tokens);
        row.model = model.clone();
        row.session_id = session.clone();
        row.project_path = cwd.clone();
        row.workspace_path = cwd.clone();
        row.identify(&format!(
            "{time}:{model}:{}:{}",
            serde_json::to_string(&row.tokens).unwrap(),
            count.total
        ));
        out.records.push(row);
    }
}
fn open_code(v: &Value, path: &Path, source: &Source) -> Option<Record> {
    let t = &v["tokens"];
    if !t.is_object() || !v["modelID"].is_string() || !v["providerID"].is_string() {
        return None;
    }
    let tokens = TokenCounts {
        input_tokens: number(&t["input"]),
        output_tokens: number(&t["output"]),
        cache_read_tokens: number(&t["cache"]["read"]),
        cache_creation_tokens: number(&t["cache"]["write"]),
        reasoning_tokens: number(&t["reasoning"]),
    };
    if tokens.total() == 0 {
        return None;
    }
    let mut row = record(source, path, timestamp(&v["time"]["created"])?, tokens);
    row.provider_id = text(&v["providerID"])?;
    row.model = text(&v["modelID"])?;
    row.session_id = text(&v["sessionID"]);
    row.cost_usd = v["cost"].as_f64();
    row.project_path = text(&v["path"]["cwd"]).or_else(|| text(&v["cwd"]));
    row.workspace_path = row.project_path.clone();
    row.identify(v["id"].as_str().unwrap_or(&path.to_string_lossy()));
    Some(row)
}
fn read_amp(v: &Value, path: &Path, source: &Source, out: &mut Parsed) {
    for event in v["usageLedger"]["events"].as_array().into_iter().flatten() {
        let Some(time) = timestamp(&event["timestamp"]) else {
            out.skipped += 1;
            continue;
        };
        let assistant = v["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| m["role"] == "assistant" && m["messageId"] == event["toMessageId"]);
        let usage = assistant.map(|m| &m["usage"]);
        let get = |key| usage.map(|u| number(&u[key])).unwrap_or(0);
        let tokens = TokenCounts {
            input_tokens: number(&event["tokens"]["input"]),
            output_tokens: number(&event["tokens"]["output"]),
            cache_read_tokens: get("cacheReadInputTokens"),
            cache_creation_tokens: get("cacheCreationInputTokens"),
            reasoning_tokens: 0,
        };
        if tokens.total() == 0 {
            continue;
        }
        let mut row = record(source, path, time, tokens);
        row.model = text(&event["model"]).unwrap_or(row.model);
        row.session_id = text(&v["id"]);
        row.project_path = text(&v["cwd"]);
        row.workspace_path = row.project_path.clone();
        row.identify(&format!("{}:{time}:{}", v["id"], event["toMessageId"]));
        out.records.push(row);
    }
}
fn read_pi(lines: impl Iterator<Item = Value>, path: &Path, source: &Source, out: &mut Parsed) {
    let mut cwd = None;
    for v in lines {
        if v["type"] == "session" {
            cwd = text(&v["cwd"]).or(cwd);
        }
        let m = &v["message"];
        let u = &m["usage"];
        if m["role"] != "assistant" || !u["input"].is_number() || !u["output"].is_number() {
            continue;
        }
        let Some(time) = timestamp(&v["timestamp"]) else {
            out.skipped += 1;
            continue;
        };
        let tokens = TokenCounts {
            input_tokens: number(&u["input"]),
            output_tokens: number(&u["output"]),
            cache_read_tokens: number(&u["cacheRead"]),
            cache_creation_tokens: number(&u["cacheWrite"]),
            reasoning_tokens: 0,
        };
        let mut row = record(source, path, time, tokens);
        row.model = text(&m["model"]).unwrap_or(row.model);
        row.session_id = file_session(path);
        row.project_path = cwd.clone();
        row.workspace_path = cwd.clone();
        row.cost_usd = u["cost"]["total"].as_f64();
        row.identify(&format!(
            "{:?}:{time}:{}",
            row.session_id,
            serde_json::to_string(&row.tokens).unwrap()
        ));
        out.records.push(row);
    }
}
fn read_grok(lines: impl Iterator<Item = Value>, path: &Path, source: &Source, out: &mut Parsed) {
    let summary = path
        .parent()
        .map(|p| p.join("summary.json"))
        .and_then(|p| fs::File::open(p).ok())
        .and_then(|f| serde_json::from_reader::<_, Value>(f).ok())
        .unwrap_or(Value::Null);
    for v in lines {
        let p = &v["params"];
        let u = &p["update"];
        if u["sessionUpdate"] != "subagent_finished" {
            continue;
        }
        let tokens = number(&u["tokens_used"]);
        if tokens == 0 {
            continue;
        }
        let Some(time) = timestamp(&v["timestamp"]).or_else(|| timestamp(&summary["created_at"]))
        else {
            out.skipped += 1;
            continue;
        };
        let mut row = record(
            source,
            path,
            time,
            TokenCounts {
                input_tokens: tokens,
                ..Default::default()
            },
        );
        row.model = text(&summary["current_model_id"]).unwrap_or_else(|| "grok-build".into());
        row.session_id = text(&p["sessionId"]);
        row.project_path = text(&summary["info"]["cwd"]);
        row.workspace_path = row.project_path.clone();
        row.confidence = "estimated".into();
        row.identify(&format!(
            "{:?}:{time}:{}:{tokens}",
            row.session_id,
            u.get("child_session_id")
                .or_else(|| u.get("subagent_id"))
                .unwrap_or(&Value::Null)
        ));
        out.records.push(row);
    }
}
fn read_native(lines: impl Iterator<Item = Value>, path: &Path, source: &Source, out: &mut Parsed) {
    let mut model = "unknown".to_owned();
    let mut cwd = None;
    let mut native = false;
    for v in lines {
        let e = &v["event"];
        if e["type"] == "sessionStarted" {
            model = text(&e["model"]).unwrap_or(model);
            cwd = text(&e["cwd"]).or(cwd);
            native = e["harness"] == "zuse";
        }
        if e["type"] != "usage" || !native {
            continue;
        }
        let Some(time) = timestamp(&v["at"]) else {
            out.skipped += 1;
            continue;
        };
        let mut row = record(
            source,
            path,
            time,
            TokenCounts {
                input_tokens: number(&e["inputTokens"]),
                output_tokens: number(&e["outputTokens"]),
                ..Default::default()
            },
        );
        row.model = model.clone();
        row.session_id = file_session(path);
        row.project_path = cwd.clone();
        row.workspace_path = cwd.clone();
        row.confidence = "partial".into();
        row.identify(&format!("journal:{}:{}", path.display(), v["seq"]));
        out.records.push(row);
    }
}
fn read_database(
    path: &Path,
    source: &Source,
    out: &mut Parsed,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(std::time::Duration::from_millis(500))?;
    if source.format == Format::OpenCode {
        let mut statement = db.prepare("SELECT id, session_id, data FROM message")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for (id, session, data) in rows.flatten() {
            if let Ok(mut value) = serde_json::from_str::<Value>(&data) {
                if !value.is_object() {
                    continue;
                }
                value["id"] = id.into();
                if value["sessionID"].is_null() {
                    value["sessionID"] = session.into();
                }
                if let Some(record) = open_code(&value, path, source) {
                    out.records.push(record);
                }
            }
        }
    } else {
        let mut statement=db.prepare("SELECT m.id,m.session_id,m.content_json,m.created_at,s.provider_id,s.model,p.path FROM messages m LEFT JOIN sessions s ON s.id=m.session_id LEFT JOIN projects p ON p.id=s.project_id WHERE m.kind='usage' ORDER BY m.created_at")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let content: String = row.get(2)?;
            let Ok(v) = serde_json::from_str::<Value>(&content) else {
                continue;
            };
            let time = row
                .get::<_, String>(3)
                .ok()
                .and_then(|s| timestamp(&Value::String(s)))
                .or_else(|| {
                    row.get::<_, i64>(3)
                        .ok()
                        .and_then(|n| timestamp(&Value::from(n)))
                });
            let Some(time) = time else {
                out.skipped += 1;
                continue;
            };
            let tokens = TokenCounts {
                input_tokens: number(&v["inputTokens"]),
                output_tokens: number(&v["outputTokens"]),
                cache_read_tokens: number(&v["cacheReadTokens"]),
                cache_creation_tokens: number(&v["cacheCreationTokens"]),
                reasoning_tokens: number(&v["reasoningTokens"]),
            };
            let mut record = record(source, path, time, tokens);
            record.session_id = row.get(1)?;
            record.provider_id = row
                .get::<_, Option<String>>(4)?
                .unwrap_or_else(|| "zuse".into());
            record.model = text(&v["model"])
                .or(row.get::<_, Option<String>>(5)?)
                .unwrap_or(record.model);
            record.project_path = row.get(6)?;
            record.workspace_path = record.project_path.clone();
            record.cost_usd = v["costUsd"].as_f64();
            record.identify(&row.get::<_, String>(0)?);
            out.records.push(record);
        }
    }
    Ok(())
}
