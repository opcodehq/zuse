//! Projections never rescan files, change source records, or treat unknown costs as zero.
use crate::{Data, Record};
use chrono::{DateTime, Local, Utc};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::Path,
};
use zuse_protocol::usage::*;

#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    /// `Some([])` is a project with no matching checkout, not global usage.
    pub project_paths: Option<Vec<String>>,
    pub timezone: Option<String>,
}
impl Selection {
    pub fn validate(&self) -> Result<(), String> {
        if self.since.zip(self.until).is_some_and(|(a, b)| a > b) {
            return Err("Usage start must precede its end.".into());
        }
        if let Some(zone) = &self.timezone {
            zone.parse::<chrono_tz::Tz>()
                .map_err(|_| "Unknown usage timezone.")?;
        }
        Ok(())
    }
    fn contains(&self, row: &Record) -> bool {
        !row.possible_duplicate
            && self.since.is_none_or(|since| row.ended_at >= since)
            && self.until.is_none_or(|until| row.started_at <= until)
            && self.project_paths.as_ref().is_none_or(|roots| {
                roots.iter().any(|root| {
                    [row.project_path.as_deref(), row.workspace_path.as_deref()]
                        .into_iter()
                        .flatten()
                        .any(|path| {
                            let path = Path::new(path);
                            let root = Path::new(root);
                            path.starts_with(root) || root.starts_with(path)
                        })
                })
            })
    }
    fn day(&self, time: DateTime<Utc>) -> String {
        match self
            .timezone
            .as_deref()
            .and_then(|zone| zone.parse::<chrono_tz::Tz>().ok())
        {
            Some(zone) => time.with_timezone(&zone).format("%Y-%m-%d").to_string(),
            None => time.with_timezone(&Local).format("%Y-%m-%d").to_string(),
        }
    }
}
fn summarize(rows: &[&Record]) -> Summary {
    let mut summary = Summary::default();
    let mut known = 0;
    let mut cost = 0.;
    for row in rows {
        summary.tokens.add(&row.tokens);
        summary.record_count += 1;
        summary.possible_duplicate_count += usize::from(row.possible_duplicate);
        if let Some(value) = row.cost_usd.filter(|v| v.is_finite() && *v >= 0.) {
            cost += value;
            known += 1;
        }
    }
    summary.cost_usd = (known > 0).then_some(cost);
    summary.cost_status = if known == rows.len() {
        CostStatus::Known
    } else if known == 0 {
        CostStatus::Unknown
    } else {
        CostStatus::Partial
    };
    summary
}
fn groups(
    rows: &[&Record],
    key: impl Fn(&Record) -> String,
    label: impl Fn(&Record) -> String,
) -> Vec<Group> {
    let mut buckets: BTreeMap<String, Vec<&Record>> = BTreeMap::new();
    for row in rows {
        buckets.entry(key(row)).or_default().push(row);
    }
    let mut result: Vec<_> = buckets
        .into_iter()
        .map(|(key, rows)| Group {
            key,
            label: label(rows[0]),
            started_at: rows.iter().map(|r| r.started_at).min(),
            ended_at: rows.iter().map(|r| r.ended_at).max(),
            source_ids: rows
                .iter()
                .map(|r| r.source_id.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            summary: summarize(&rows),
        })
        .collect();
    result.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.label.cmp(&b.label))
    });
    result
}
fn session_key(row: &Record) -> String {
    row.session_id.clone().unwrap_or_else(|| row.id.clone())
}
fn sessions(rows: &[&Record]) -> Vec<Group> {
    groups(rows, session_key, |r| {
        r.session_id
            .clone()
            .unwrap_or_else(|| format!("{} {}", r.source_label, r.model))
    })
}
fn by_source(rows: &[&Record]) -> Vec<Group> {
    groups(rows, |r| r.source_id.clone(), |r| r.source_label.clone())
}
fn by_model(rows: &[&Record]) -> Vec<Group> {
    groups(rows, |r| r.model.clone(), |r| r.model.clone())
}
fn by_project(rows: &[&Record]) -> Vec<Group> {
    groups(
        rows,
        |r| {
            r.project_path
                .clone()
                .or(r.workspace_path.clone())
                .unwrap_or_else(|| "unknown".into())
        },
        |r| {
            r.project_path
                .as_deref()
                .or(r.workspace_path.as_deref())
                .and_then(|p| Path::new(p).file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Unknown project".into())
        },
    )
}
impl Data {
    pub fn overview(&self, selection: &Selection, now: DateTime<Utc>) -> Result<Overview, String> {
        selection.validate()?;
        let rows: Vec<_> = self
            .records
            .iter()
            .filter(|r| selection.contains(r))
            .collect();
        let previous = selection.since.map(|since| {
            let span = selection.until.unwrap_or(now).signed_duration_since(since);
            let mut prior = selection.clone();
            prior.since = since.checked_sub_signed(span);
            prior.until = Some(since);
            self.records
                .iter()
                .filter(|r| prior.contains(r))
                .collect::<Vec<_>>()
        });
        Ok(Overview {
            bucket: "daily".into(),
            generated_at: now,
            summary: summarize(&rows),
            session_count: sessions(&rows).len(),
            previous_summary: previous.as_ref().map(|rows| summarize(rows)),
            previous_session_count: previous.as_ref().map(|rows| sessions(rows).len()),
            groups: groups(
                &rows,
                |r| selection.day(r.started_at),
                |r| selection.day(r.started_at),
            ),
            by_source: by_source(&rows),
            by_model: by_model(&rows),
            by_project: by_project(&rows),
            previous_by_source: previous
                .as_ref()
                .map(|rows| by_source(rows))
                .unwrap_or_default(),
            previous_by_model: previous
                .as_ref()
                .map(|rows| by_model(rows))
                .unwrap_or_default(),
            previous_by_project: previous
                .as_ref()
                .map(|rows| by_project(rows))
                .unwrap_or_default(),
            sources: self.sources.clone(),
        })
    }
    pub fn sessions(
        &self,
        selection: &Selection,
        request: &Request,
    ) -> Result<SessionsPage, String> {
        selection.validate()?;
        let records: Vec<_> = self
            .records
            .iter()
            .filter(|r| selection.contains(r))
            .filter(|r| {
                request
                    .provider_id
                    .as_deref()
                    .is_none_or(|id| id == r.provider_id)
            })
            .collect();
        let needle = request.query.trim().to_lowercase();
        let mut rows: Vec<_> = sessions(&records)
            .into_iter()
            .filter(|g| needle.is_empty() || g.label.to_lowercase().contains(&needle))
            .collect();
        rows.sort_by(|a, b| {
            match request.sort {
                SessionSort::Tokens => b.summary.tokens.total().cmp(&a.summary.tokens.total()),
                SessionSort::Cost => b
                    .summary
                    .cost_usd
                    .unwrap_or(0.)
                    .total_cmp(&a.summary.cost_usd.unwrap_or(0.)),
                SessionSort::LastActive => b.ended_at.cmp(&a.ended_at),
            }
            .then_with(|| a.key.cmp(&b.key))
        });
        let total = rows.len();
        let offset = request.offset.min(total);
        let end = offset
            .saturating_add(request.limit.unwrap_or(10).clamp(1, 100))
            .min(total);
        Ok(SessionsPage {
            rows: rows.drain(offset..end).collect(),
            total,
            next_offset: (end < total).then_some(end),
        })
    }
    /// Keep repeat imports idempotent and flag overlapping exports across sources.
    pub fn deduplicate(&mut self) {
        let mut identities = HashSet::new();
        self.records.retain(|r| identities.insert(r.id.clone()));
        self.records.sort_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.source_id.cmp(&b.source_id))
                .then_with(|| a.id.cmp(&b.id))
        });
        let mut seen = HashSet::new();
        for row in &mut self.records {
            let key = serde_json::to_string(&(
                &row.provider_id,
                &row.model,
                &row.session_id,
                &row.project_path,
                row.started_at,
                &row.tokens,
            ))
            .unwrap();
            row.possible_duplicate = !seen.insert(key);
        }
    }
}
