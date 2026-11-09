//! Agent-facing guidance: when to show a UI block, the syntax rules, worked
//! examples, and the component catalogue lines.

use crate::catalogue::component_lines;

/// Shared response guidance for the workspace prompt and the `show_ui` tool.
pub const RESPONSE_INSTRUCTIONS: &str = "Zuse can render UI blocks directly in chat. Call show_ui proactively when a visual makes the answer easier to scan; the user does not need to ask for UI.
For a final answer with multiple changes, checks, findings, or options, make one compact visual summary the main answer. Lead with one sentence stating the outcome, then emit the summary before ending the turn. Put only essential links or next steps in the closing text. A simple fix, single fact, or short conversation can stay plain text; honor the user's requested format.
Choose the visual around the main question: numerical comparisons or before/after measurements → BarChart; changes over time → LineChart; part-to-whole breakdowns → DonutChart; architecture, relationships, or data flow → Flow; plans or rollout status → Steps. When the answer contains comparable measured values, lead with the appropriate chart rather than a table of numbers. For qualitative explanations, use Flow or Steps when connections or sequence matter; they need no invented metrics. Use Grid of Stat for a few standalone metrics, Table for exact lookups or qualitative feature matrices, and Tabs for secondary views. Completed work can use a short List of user-visible changes and KeyValue for checks. Group related components in a Card only when useful.
Avoid defaulting to tables, quote-like Callouts, or text-only Cards for every answer. Use Callout sparingly for a material warning or caveat, not as a routine recap. Do not force a graph into a simple answer or invent values to create visual variety.
Keep the chat clean: use a short title, concise labels, shallow sections, and at most 2–4 headline metrics. Group related content in a single block; use Tabs only for secondary detail, keeping the outcome and blockers visible. Match the visual to the available evidence rather than filling a dashboard with decoration. Include units and time windows for measurements, distinguish estimates from observations, and surface missing or conflicting data. Report checks as passed, failed, or not run with the reason; a completed task does not imply a successful check. Keep tables to a few concise columns with short cells; use a chart for numerical comparisons and move long explanations into a separate section. Use only supported components and verified data; do not invent metrics, progress percentages, or capabilities.
The visual replaces the equivalent prose recap. State next steps in prose rather than buttons or suggested prompts. Use Form only when blocked on several user answers. show_ui is the default for charts, tables, diagrams, and dashboards; use `show_visual` for custom drawing, 3D scenes, shaders, or live simulations the components cannot express.
Each successful show_ui call appends a complete block, so compose one finished summary rather than repeatedly emitting partial versions. If validation fails, correct the spec and retry; if rendering remains unavailable, give a concise text answer with the same findings.";

/// The worked examples embedded in [`tool_description`]. Each one parses.
pub const EXAMPLES: [(&str, &str); 4] = [
    (
        "measured before/after comparison (replace with actual measurements)",
        r#"root = BarChart("Page load · same fixture and environment", [{label: "Before", value: 820}, {label: "After", value: 540}], "ms")"#,
    ),
    (
        "an architecture explanation without numerical data",
        r#"root = Flow([{label: "GitHub webhook"}, {label: "Control plane", detail: "Leases, retries", highlight: true}, {label: "Isolated worker"}, {label: "Check + comments"}], "The control plane dispatches reviews to isolated workers.")"#,
    ),
    (
        "a dashboard",
        r#"root = Card([metrics, caveat, breakdown, trend], "Usage · Oct 1–2 · USD")
metrics = Grid([Stat("Recorded spend", "$120", "Request records"), Stat("Requests", "3,000", "Two days")], 2)
caveat = Callout("Budget data is unavailable; no budget projection is shown.", "warn", "Missing source")
breakdown = DonutChart("Spend by model", [{label: "Model A", value: 80}, {label: "Model B", value: 40}], "USD")
trend = LineChart("Daily spend", [{label: "Oct 1", value: 50}, {label: "Oct 2", value: 70}], "USD")"#,
    ),
    (
        "a final coding summary (replace the example facts with actual results)",
        r#"root = Card([changes, checks, caveat], "Reconnect recovery")
changes = List(["Keeps partial replies after reconnect", "Resumes the existing session"])
checks = KeyValue([{key: "Biome", value: "Passed"}, {key: "Types", value: "Passed"}, {key: "Recovery tests", value: "12 passed"}])
caveat = Callout("Desktop smoke test not run: this environment has no desktop session.", "warn", "Verification limit")"#,
    ),
];

/// Full description for the `show_ui` tool: purpose, response guidance,
/// syntax rules, examples, and the catalogue (generated from the same table
/// that validates specs).
pub fn tool_description() -> String {
    let examples = EXAMPLES
        .iter()
        .map(|(title, spec)| format!("Example — {title}:\n{spec}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "Render visuals inline in your reply: diagrams, comparisons, charts, dashboards, and plans. This is Zuse's default way to show anything visual; prefer it over show_visual whenever the components fit. The spec is UI block spec text (syntax below). It renders directly in the chat (no card or background), between your text messages.

Display-only snapshots: no tools, callbacks, or live data. A Form submission sends its answers as the user's next message.

{RESPONSE_INSTRUCTIONS}

Syntax rules:
- One statement per line: name = Expression
- root = ... is REQUIRED — it is the top-level component
- Component arguments are POSITIONAL only: Stat(\"Downloads\", \"12k\") — never name: value
- Strings are double-quoted; nest components inside array args: Card([s1, s2])
- Card title: Card([children], \"Title\"); do not use CardHeader. KeyValue entries are objects with key and value strings, never tuples or label fields.
- Define parts on their own lines and reference them by name; every defined name (except root) must be reachable from root
- Pass null for an optional arg you want to skip
- Use literal data only. No reactive state, expressions, Query(), Mutation(), or actions.
- Fields (Input, TextArea, Select, RadioGroup, Checkbox, Slider) must be inside a Form, with unique names. Forms cannot nest.
- Keep specs under 32,768 characters. Containers hold at most 64 components; charts hold 1-100 points; tables hold 1-20 columns and at most 200 rows.

{examples}

Components (signature — purpose):
{lines}

Succeeds once the block is rendered; a validation error lists what to fix (with line numbers) and nothing renders.",
        lines = component_lines(),
    )
}
