//! Test-only measurement: count SQL statements per instrumented request.
//! Run `aaa_sql_census_install`, the tests to measure, then
//! `zzz_sql_census_report` in one process with `--test-threads=1`.
use std::collections::BTreeMap;
use std::sync::Mutex;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;

#[derive(Default)]
struct Tally {
    calls: u64,
    statements: u64,
    begins: u64,
    commits: u64,
    by_kind: BTreeMap<String, u64>,
}
static TALLY: Mutex<BTreeMap<String, Tally>> = Mutex::new(BTreeMap::new());

struct Operation(String);
#[derive(Default)]
struct OperationVisit(Option<String>);
impl Visit for OperationVisit {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "operation" {
            self.0 = Some(value.to_owned());
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "operation" {
            self.0 = Some(format!("{value:?}").trim_matches('"').to_owned());
        }
    }
}
#[derive(Default)]
struct SummaryVisit(Option<String>);
impl Visit for SummaryVisit {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "summary" || field.name() == "db.statement" {
            self.0.get_or_insert_with(|| value.to_owned());
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "summary" {
            self.0.get_or_insert_with(|| format!("{value:?}"));
        }
    }
}

struct Census;
impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Census {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut visit = OperationVisit::default();
        attrs.record(&mut visit);
        if let (Some(op), Some(span)) = (visit.0, ctx.span(id)) {
            let key = format!("{}:{}", span.name(), op);
            TALLY.lock().unwrap().entry(key.clone()).or_default().calls += 1;
            span.extensions_mut().insert(Operation(key));
        }
    }
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        let key = ctx
            .event_scope(event)
            .and_then(|scope| {
                scope
                    .from_root()
                    .filter_map(|s| s.extensions().get::<Operation>().map(|o| o.0.clone()))
                    .last()
            })
            .unwrap_or_else(|| "(outside instrumented requests)".into());
        let mut visit = SummaryVisit::default();
        event.record(&mut visit);
        let summary = visit.0.unwrap_or_default().to_lowercase();
        let kind = summary.split_whitespace().next().unwrap_or("?").trim_matches('"').to_owned();
        let mut tally = TALLY.lock().unwrap();
        let t = tally.entry(key).or_default();
        t.statements += 1;
        match kind.as_str() {
            "begin" => t.begins += 1,
            "commit" | "rollback" => t.commits += 1,
            _ => {}
        }
        *t.by_kind.entry(kind).or_default() += 1;
    }
}

#[test]
fn install() {
    let filter = tracing_subscriber::filter::Targets::new()
        .with_target("sqlx::query", tracing::Level::TRACE)
        .with_default(tracing::Level::TRACE);
    let _ = tracing_subscriber::registry()
        .with(Census.with_filter(filter))
        .try_init();
}

pub(crate) fn report() {
    let tally = TALLY.lock().unwrap();
    println!("SQLCENSUS operation\tcalls\tstatements\tper_call\tbegin\tcommit\tkinds");
    for (op, t) in tally.iter() {
        if t.statements == 0 {
            continue;
        }
        let per = t.statements as f64 / t.calls.max(1) as f64;
        println!(
            "SQLCENSUS {op}\t{}\t{}\t{per:.1}\t{}\t{}\t{:?}",
            t.calls, t.statements, t.begins, t.commits, t.by_kind
        );
    }
}
