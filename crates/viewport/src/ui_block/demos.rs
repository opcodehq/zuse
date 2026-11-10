//! Fixture specs that exercise every component and edge case: the agent
//! prompt's worked examples plus scaling, empty, responsive and fallback
//! cases.

use zuse_ui_block::EXAMPLES;

static DEMOS: [(&str, &str); 14] = [
    EXAMPLES[0],
    EXAMPLES[1],
    EXAMPLES[2],
    EXAMPLES[3],
    (
        "bar chart with negative values",
        r#"root = BarChart("Latency change vs. last week", [{label: "API", value: -42}, {label: "Search", value: 18.5}, {label: "Uploads", value: -7}, {label: "Auth", value: 0}, {label: "Billing", value: 64}], "ms")"#,
    ),
    (
        "donut chart with a zero total",
        r#"root = DonutChart("Failed runs by area", [{label: "Build", value: 0}, {label: "Tests", value: 0}, {label: "Deploy", value: 0}], "runs")"#,
    ),
    (
        "line chart with negative values",
        r#"root = LineChart("Net queue growth · last 12 hours", [{label: "00:00", value: 12}, {label: "01:00", value: 18}, {label: "02:00", value: 9}, {label: "03:00", value: -4}, {label: "04:00", value: -11}, {label: "05:00", value: -6}, {label: "06:00", value: 3}, {label: "07:00", value: 21}, {label: "08:00", value: 34}, {label: "09:00", value: 28}, {label: "10:00", value: 30}, {label: "11:00", value: 25}], "jobs")"#,
    ),
    (
        "steps in every status",
        r#"root = Steps([{label: "Plan the migration", status: "done", detail: "Schema reviewed with the data team"}, {label: "Backfill rows", status: "active", detail: "62% of 4.1M rows"}, {label: "Switch reads to the new table", status: "pending"}, {label: "Drop the old table", status: "failed", detail: "Blocked by a foreign key from audit_log"}])"#,
    ),
    (
        "tabs with a table",
        r#"root = Tabs([{label: "Summary", content: summary}, {label: "Services", content: services}])
summary = Text("Three services were upgraded.\nOne stays pinned until its client library ships a fix.")
services = Table(["Service", "Version", "Status"], [["api", "2.4.1", "Upgraded"], ["worker", "2.4.1", "Upgraded"], ["billing", "2.3.9", "Pinned"]])"#,
    ),
    (
        "wide table with long cells",
        r#"root = Table(["Check", "Owner", "Environment", "Result", "Duration", "Notes"], [["Unit tests", "platform", "CI · linux-x64", "Passed", "4m 12s", "All 1,284 tests passed; three were retried once after a flaky network fixture timed out."], ["Integration", "payments", "staging", "Failed", "11m 03s", "Webhook signature verification rejected replayed events from the sandbox provider."], ["Browser smoke", "web", "preview-1432", "Not run", "—", "Runner pool exhausted; rerun scheduled after the nightly build finishes."]])"#,
    ),
    (
        "form with every field type",
        r#"root = Form([name, notes, region, plan, terms, budget], "Request access", "Workspace request")
name = Input("name", "Workspace name", "acme-prod")
notes = TextArea("notes", "What will you use it for?", "A sentence or two")
region = Select("region", "Region", ["us-east", "eu-west", "ap-south"], "eu-west")
plan = RadioGroup("plan", "Plan", ["Starter", "Team", "Enterprise"], "Team")
terms = Checkbox("terms", "I accept the usage policy", true)
budget = Slider("budget", "Monthly budget (USD)", 0, 500, 120, 10)"#,
    ),
    (
        "flow with a highlighted step",
        r#"root = Flow([{label: "Client", detail: "Browser or desktop"}, {label: "Edge", detail: "TLS, rate limits"}, {label: "Gateway", detail: "Auth, routing", highlight: true}, {label: "Queue"}, {label: "Workers", detail: "Autoscaled"}, {label: "Storage", detail: "Postgres + blobs"}], "Requests enter at the edge; the gateway is where this change lands.")"#,
    ),
    (
        "grid of stats, badges and progress",
        r#"root = Card([stats, health], "Release 0.26 readiness")
stats = Grid([Stat("Open issues", "14", "3 blockers"), Stat("Merged PRs", "87"), Stat("Crash-free", "99.6%", "Last 7 days"), Stat("Downloads", "12.4K")], 4)
health = Grid([Badge("Docs ready", "good"), Badge("Perf regressions", "warn"), Badge("Signing broken", "bad"), Badge("Beta"), Progress("Translations", 72), Progress("QA pass", 100)], 3)"#,
    ),
    (
        "invalid spec (fallback)",
        r#"root = Chart("Revenue", [1, 2, 3])"#,
    ),
];

/// `(title, source)` fixtures covering every component, including an
/// invalid spec that renders the fallback.
pub fn demo_specs() -> &'static [(&'static str, &'static str)] {
    &DEMOS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_demo_parses_except_the_fallback() {
        for (title, source) in demo_specs() {
            let parsed = zuse_ui_block::parse(source);
            if title.contains("invalid") {
                assert!(parsed.is_err(), "{title} should fail");
            } else {
                assert!(parsed.is_ok(), "{title}: {:?}", parsed.err());
            }
        }
    }
}
