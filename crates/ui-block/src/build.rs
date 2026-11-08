//! Validate the resolved program against the catalogue and build the typed
//! tree. Shape checks (types, lengths, ranges, enums) are driven entirely by
//! the catalogue table; rules that span arguments (table cell counts, option
//! membership, slider ranges, form scoping) live in the per-component code.

use crate::catalogue::{self, ComponentDef, Param, Ty};
use crate::error::Issue;
use crate::model::{
    ChartPoint, Component, FlowNode, FollowUp, KeyValuePair, Step, StepStatus, Tab, Tone,
};
use crate::resolve::Program;
use crate::syntax::{Expr, Node};

pub(crate) fn build(program: &Program) -> Result<Component, Vec<Issue>> {
    let mut builder = Builder {
        program,
        issues: Vec::new(),
    };
    let root = builder.component(program.root(), "root", None);
    match root {
        Some(root) if builder.issues.is_empty() => Ok(root),
        _ => {
            if builder.issues.is_empty() {
                builder
                    .issues
                    .push(Issue::general("root must be a component."));
            }
            Err(builder.issues)
        }
    }
}

/// Field names already used by the enclosing Form, or `None` outside a Form.
type FormScope<'s> = Option<&'s mut Vec<String>>;

struct Builder<'p> {
    program: &'p Program,
    issues: Vec<Issue>,
}

fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

fn plural(n: usize, one: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {one}s")
    }
}

fn describe(node: &Node) -> String {
    match &node.expr {
        Expr::Str(value) => {
            let mut shown: String = value.chars().take(40).collect();
            if shown.len() < value.len() {
                shown.push('…');
            }
            format!(
                "string {}",
                serde_json::to_string(&shown).unwrap_or_default()
            )
        }
        Expr::Num(value) => format!("number {}", crate::js_number(*value)),
        Expr::Bool(value) => format!("boolean {value}"),
        Expr::Null => "null".into(),
        Expr::Arr(_) => "an array".into(),
        Expr::Obj(_) => "an object".into(),
        Expr::Call { name, .. } => format!("component {name}"),
        Expr::Ref(name) => format!("reference {name}"),
    }
}

fn is_field_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && name.len() <= 40
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl<'p> Builder<'p> {
    fn node(&self, node: &'p Node) -> &'p Node {
        self.program.deref(node)
    }

    fn issue(&mut self, line: usize, message: String) {
        self.issues.push(Issue::at(line, message));
    }

    // ── Catalogue-driven shape checks ──────────────────────────────────────

    /// Check `node` against `ty`, reporting every violation under
    /// `{component} {path}`. Nested components are only checked for being
    /// components; they validate themselves when built.
    fn check(&mut self, node: &'p Node, ty: &Ty, component: &str, path: &str) -> bool {
        let node = self.node(node);
        let line = node.line;
        let mismatch = |this: &mut Self, expected: String| {
            this.issue(
                line,
                format!(
                    "{component} {path}: expected {expected}, got {}.",
                    describe(node)
                ),
            );
            false
        };
        match ty {
            Ty::Str { min, max } => {
                let Expr::Str(value) = &node.expr else {
                    return mismatch(self, "string".into());
                };
                let len = utf16_len(value);
                if len < *min {
                    let message = if *min == 1 {
                        format!("{component} {path}: must not be empty.")
                    } else {
                        format!("{component} {path}: must be at least {min} characters.")
                    };
                    self.issue(line, message);
                    return false;
                }
                if let Some(max) = max
                    && len > *max
                {
                    self.issue(
                        line,
                        format!(
                            "{component} {path}: must be at most {max} characters (got {len}); shorten it."
                        ),
                    );
                    return false;
                }
                true
            }
            Ty::FieldName => {
                let Expr::Str(value) = &node.expr else {
                    return mismatch(self, "string".into());
                };
                if !is_field_name(value) {
                    self.issue(
                        line,
                        format!(
                            "{component} {path}: {} is not a valid field name — start with a letter, then use letters, digits, - or _ (at most 40 characters).",
                            serde_json::to_string(value).unwrap_or_default()
                        ),
                    );
                    return false;
                }
                true
            }
            Ty::Num {
                min,
                max,
                int,
                positive,
            } => {
                let Expr::Num(value) = node.expr else {
                    return mismatch(self, "number".into());
                };
                let shown = crate::js_number(value);
                let problem = if !value.is_finite() {
                    Some("must be a finite number".to_string())
                } else if *int && value.fract() != 0.0 {
                    Some(format!("must be a whole number (got {shown})"))
                } else if *positive && value <= 0.0 {
                    Some(format!("must be greater than 0 (got {shown})"))
                } else {
                    match (min, max) {
                        (Some(lo), Some(hi)) if value < *lo || value > *hi => Some(format!(
                            "must be between {} and {} (got {shown})",
                            crate::js_number(*lo),
                            crate::js_number(*hi)
                        )),
                        (Some(lo), None) if value < *lo => Some(format!(
                            "must be at least {} (got {shown})",
                            crate::js_number(*lo)
                        )),
                        (None, Some(hi)) if value > *hi => Some(format!(
                            "must be at most {} (got {shown})",
                            crate::js_number(*hi)
                        )),
                        _ => None,
                    }
                };
                match problem {
                    Some(problem) => {
                        self.issue(line, format!("{component} {path}: {problem}."));
                        false
                    }
                    None => true,
                }
            }
            Ty::Bool => match node.expr {
                Expr::Bool(_) => true,
                _ => mismatch(self, "true or false".into()),
            },
            Ty::Enum(values) => {
                let expected = values
                    .iter()
                    .map(|value| format!("\"{value}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                match &node.expr {
                    Expr::Str(value) if values.contains(&value.as_str()) => true,
                    _ => mismatch(self, format!("one of {expected}")),
                }
            }
            Ty::Array { item, min, max } => {
                let Expr::Arr(items) = &node.expr else {
                    return mismatch(self, format!("an array ({})", ty.label()));
                };
                if items.len() < *min {
                    self.issue(
                        line,
                        format!(
                            "{component} {path}: needs at least {} (got {}).",
                            plural(*min, "item"),
                            items.len()
                        ),
                    );
                    return false;
                }
                if let Some(max) = max
                    && items.len() > *max
                {
                    self.issue(
                        line,
                        format!(
                            "{component} {path}: allows at most {} (got {}); trim or split the data.",
                            plural(*max, "item"),
                            items.len()
                        ),
                    );
                    return false;
                }
                let mut ok = true;
                for (i, item_node) in items.iter().enumerate() {
                    ok &= self.check(item_node, item, component, &format!("{path}[{i}]"));
                }
                ok
            }
            Ty::Object(fields) => {
                let Expr::Obj(_) = &node.expr else {
                    return mismatch(self, format!("an object {}", ty.label()));
                };
                let mut ok = true;
                for field in *fields {
                    let field_path = format!("{path}.{}", field.name);
                    match self.entry(node, field.name) {
                        Some(value) => ok &= self.check(value, &field.ty, component, &field_path),
                        None if field.required => {
                            self.issue(
                                line,
                                format!(
                                    "{component} {path}: missing required key `{}` ({}).",
                                    field.name,
                                    field.ty.label()
                                ),
                            );
                            ok = false;
                        }
                        None => {}
                    }
                }
                ok
            }
            Ty::Component => match node.expr {
                Expr::Call { .. } => true,
                _ => {
                    self.issue(
                        line,
                        format!(
                            "{component} {path}: must be a component like Text(\"…\"), got {}.",
                            describe(node)
                        ),
                    );
                    false
                }
            },
        }
    }

    /// Object entry by key, dereferenced; `null` counts as absent.
    fn entry(&self, object: &'p Node, key: &str) -> Option<&'p Node> {
        let Expr::Obj(entries) = &self.node(object).expr else {
            return None;
        };
        let value = self.node(&entries.iter().find(|(k, _)| k == key)?.1);
        (!matches!(value.expr, Expr::Null)).then_some(value)
    }

    // ── Typed accessors (only called after `check` passed) ─────────────────

    fn str(&self, node: &'p Node) -> String {
        match &self.node(node).expr {
            Expr::Str(value) => value.clone(),
            _ => unreachable!("checked as string"),
        }
    }

    fn num(&self, node: &'p Node) -> f64 {
        match self.node(node).expr {
            Expr::Num(value) => value,
            _ => unreachable!("checked as number"),
        }
    }

    fn bool(&self, node: &'p Node) -> bool {
        matches!(self.node(node).expr, Expr::Bool(true))
    }

    fn items(&self, node: &'p Node) -> &'p [Node] {
        match &self.node(node).expr {
            Expr::Arr(items) => items,
            _ => unreachable!("checked as array"),
        }
    }

    fn strings(&self, node: &'p Node) -> Vec<String> {
        self.items(node).iter().map(|item| self.str(item)).collect()
    }

    fn key_str(&self, object: &'p Node, key: &str) -> String {
        self.entry(object, key)
            .map(|value| self.str(value))
            .unwrap_or_default()
    }

    fn key_opt_str(&self, object: &'p Node, key: &str) -> Option<String> {
        self.entry(object, key).map(|value| self.str(value))
    }

    fn points(&self, node: &'p Node) -> Vec<ChartPoint> {
        self.items(node)
            .iter()
            .map(|point| ChartPoint {
                label: self.key_str(point, "label"),
                value: self.entry(point, "value").map_or(0.0, |v| self.num(v)),
            })
            .collect()
    }

    // ── Components ─────────────────────────────────────────────────────────

    fn children(
        &mut self,
        node: Option<&'p Node>,
        mut scope: FormScope<'_>,
    ) -> Option<Vec<Component>> {
        let Some(node) = node else {
            return Some(Vec::new());
        };
        let mut built = Vec::new();
        let mut ok = true;
        for (i, child) in self.items(node).iter().enumerate() {
            match self.component(child, &format!("children[{i}]"), scope.as_deref_mut()) {
                Some(component) => built.push(component),
                None => ok = false,
            }
        }
        ok.then_some(built)
    }

    fn component(
        &mut self,
        node: &'p Node,
        place: &str,
        mut scope: FormScope<'_>,
    ) -> Option<Component> {
        let node = self.node(node);
        let Expr::Call { name, args } = &node.expr else {
            self.issue(
                node.line,
                format!(
                    "{place} must be a component like Text(\"…\"), got {}.",
                    describe(node)
                ),
            );
            return None;
        };
        let def = catalogue::lookup(name).expect("names resolved earlier");
        let line = node.line;
        let args = self.arguments(def, args, line)?;
        let arg = |i: usize| args[i];

        if def.field {
            let field_name = self.str(arg(0).expect("required"));
            match scope.as_deref_mut() {
                None => {
                    self.issue(
                        line,
                        format!(
                            "{name} `{field_name}` must be inside a Form. Wrap the fields in Form([...], \"Submit\")."
                        ),
                    );
                    return None;
                }
                Some(fields) => {
                    if fields.contains(&field_name) {
                        self.issue(
                            line,
                            format!(
                                "Duplicate field name: `{field_name}`. Every field in a Form needs a unique name."
                            ),
                        );
                        return None;
                    }
                    fields.push(field_name);
                }
            }
        }

        let opt_str = |this: &Self, i: usize| arg(i).map(|n| this.str(n));
        let tone = |this: &Self, i: usize| arg(i).and_then(|n| Tone::from_name(&this.str(n)));
        let req_str = |this: &Self, i: usize| this.str(arg(i).expect("required"));

        Some(match name.as_str() {
            "Card" => Component::Card {
                children: self.children(arg(0), scope)?,
                title: opt_str(self, 1),
            },
            "Text" => Component::Text {
                text: req_str(self, 0),
            },
            "Stat" => Component::Stat {
                label: req_str(self, 0),
                value: req_str(self, 1),
                hint: opt_str(self, 2),
            },
            "KeyValue" => Component::KeyValue {
                pairs: self
                    .items(arg(0).expect("required"))
                    .iter()
                    .map(|pair| KeyValuePair {
                        key: self.key_str(pair, "key"),
                        value: self.key_str(pair, "value"),
                    })
                    .collect(),
            },
            "Table" => {
                let columns = self.strings(arg(0).expect("required"));
                let rows_node = arg(1).expect("required");
                let rows: Vec<Vec<String>> = self
                    .items(rows_node)
                    .iter()
                    .map(|row| self.strings(row))
                    .collect();
                let mut ok = true;
                for (i, row) in rows.iter().enumerate() {
                    if row.len() != columns.len() {
                        let row_line = self.node(&self.items(rows_node)[i]).line;
                        self.issue(
                            row_line,
                            format!(
                                "Table rows must have the same number of cells as columns: rows[{i}] has {} but there are {}.",
                                plural(row.len(), "cell"),
                                plural(columns.len(), "column")
                            ),
                        );
                        ok = false;
                    }
                }
                if !ok {
                    return None;
                }
                Component::Table { columns, rows }
            }
            "Progress" => Component::Progress {
                label: req_str(self, 0),
                percent: self.num(arg(1).expect("required")),
            },
            "Badge" => Component::Badge {
                label: req_str(self, 0),
                tone: tone(self, 1),
            },
            "List" => Component::List {
                items: self.strings(arg(0).expect("required")),
            },
            "Grid" => Component::Grid {
                children: self.children(arg(0), scope)?,
                columns: arg(1).map(|n| self.num(n) as u8),
            },
            "BarChart" | "LineChart" | "DonutChart" => {
                let title = req_str(self, 0);
                let points = self.points(arg(1).expect("required"));
                let unit = opt_str(self, 2);
                match name.as_str() {
                    "BarChart" => Component::BarChart {
                        title,
                        points,
                        unit,
                    },
                    "LineChart" => Component::LineChart {
                        title,
                        points,
                        unit,
                    },
                    _ => {
                        let total: f64 = points.iter().map(|point| point.value).sum();
                        if !total.is_finite() {
                            self.issue(
                                line,
                                "DonutChart points: Total must be finite; use smaller values or another unit.".into(),
                            );
                            return None;
                        }
                        Component::DonutChart {
                            title,
                            points,
                            unit,
                        }
                    }
                }
            }
            "Callout" => Component::Callout {
                text: req_str(self, 0),
                tone: tone(self, 1),
                title: opt_str(self, 2),
            },
            "Steps" => Component::Steps {
                items: self
                    .items(arg(0).expect("required"))
                    .iter()
                    .map(|item| Step {
                        label: self.key_str(item, "label"),
                        status: StepStatus::from_name(&self.key_str(item, "status"))
                            .expect("checked enum"),
                        detail: self.key_opt_str(item, "detail"),
                    })
                    .collect(),
            },
            "Flow" => Component::Flow {
                nodes: self
                    .items(arg(0).expect("required"))
                    .iter()
                    .map(|item| FlowNode {
                        label: self.key_str(item, "label"),
                        detail: self.key_opt_str(item, "detail"),
                        highlight: self.entry(item, "highlight").is_some_and(|v| self.bool(v)),
                    })
                    .collect(),
                caption: opt_str(self, 1),
            },
            "Tabs" => {
                let mut tabs = Vec::new();
                let mut ok = true;
                for (i, item) in self.items(arg(0).expect("required")).iter().enumerate() {
                    let label = self.key_str(item, "label");
                    let content = self.entry(item, "content").expect("required key");
                    match self.component(
                        content,
                        &format!("Tab content tabs[{i}]"),
                        scope.as_deref_mut(),
                    ) {
                        Some(content) => tabs.push(Tab { label, content }),
                        None => ok = false,
                    }
                }
                if !ok {
                    return None;
                }
                Component::Tabs { tabs }
            }
            "FollowUps" => Component::FollowUps {
                items: self
                    .items(arg(0).expect("required"))
                    .iter()
                    .map(|item| FollowUp {
                        label: self.key_str(item, "label"),
                        prompt: self.key_str(item, "prompt"),
                    })
                    .collect(),
            },
            "Form" => {
                if scope.is_some() {
                    self.issue(
                        line,
                        "Forms cannot be nested; put all fields in one Form.".into(),
                    );
                    return None;
                }
                let mut fields = Vec::new();
                let children = self.children(arg(0), Some(&mut fields))?;
                if fields.is_empty() {
                    self.issue(
                        line,
                        "Form needs at least one field (Input, TextArea, Select, RadioGroup, Checkbox or Slider).".into(),
                    );
                    return None;
                }
                Component::Form {
                    children,
                    submit_label: opt_str(self, 1),
                    title: opt_str(self, 2),
                }
            }
            "Input" | "TextArea" => {
                let (field_name, label) = (req_str(self, 0), req_str(self, 1));
                let (placeholder, value) = (opt_str(self, 2), opt_str(self, 3));
                if name == "Input" {
                    Component::Input {
                        name: field_name,
                        label,
                        placeholder,
                        value,
                    }
                } else {
                    Component::TextArea {
                        name: field_name,
                        label,
                        placeholder,
                        value,
                    }
                }
            }
            "Select" | "RadioGroup" => {
                let field_name = req_str(self, 0);
                let label = req_str(self, 1);
                let options = self.strings(arg(2).expect("required"));
                let value = opt_str(self, 3);
                if let Some(value) = &value
                    && !options.contains(value)
                {
                    let shown = options
                        .iter()
                        .map(|option| serde_json::to_string(option).unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.issue(
                        line,
                        format!(
                            "{name} {field_name}: value must be one of the options (got {}; options: {shown}).",
                            serde_json::to_string(value).unwrap_or_default()
                        ),
                    );
                    return None;
                }
                if name == "Select" {
                    Component::Select {
                        name: field_name,
                        label,
                        options,
                        value,
                    }
                } else {
                    Component::RadioGroup {
                        name: field_name,
                        label,
                        options,
                        value,
                    }
                }
            }
            "Checkbox" => Component::Checkbox {
                name: req_str(self, 0),
                label: req_str(self, 1),
                checked: arg(2).is_some_and(|n| self.bool(n)),
            },
            "Slider" => {
                let field_name = req_str(self, 0);
                let label = req_str(self, 1);
                let min = self.num(arg(2).expect("required"));
                let max = self.num(arg(3).expect("required"));
                let value = arg(4).map(|n| self.num(n));
                let step = arg(5).map(|n| self.num(n));
                if min >= max {
                    self.issue(
                        line,
                        format!(
                            "Slider {field_name}: min must be below max (min {}, max {}).",
                            crate::js_number(min),
                            crate::js_number(max)
                        ),
                    );
                    return None;
                }
                if let Some(value) = value
                    && (value < min || value > max)
                {
                    self.issue(
                        line,
                        format!(
                            "Slider {field_name}: value must be within min and max (got {}; range {} to {}).",
                            crate::js_number(value),
                            crate::js_number(min),
                            crate::js_number(max)
                        ),
                    );
                    return None;
                }
                Component::Slider {
                    name: field_name,
                    label,
                    min,
                    max,
                    value,
                    step,
                }
            }
            other => unreachable!("catalogue component {other} has no builder"),
        })
    }

    /// Map positional arguments onto the catalogue parameters: arity,
    /// required/`null` handling, and shape checks. Returns one dereferenced
    /// node per parameter (`None` = omitted or `null`).
    fn arguments(
        &mut self,
        def: &'static ComponentDef,
        args: &'p [Node],
        line: usize,
    ) -> Option<Vec<Option<&'p Node>>> {
        let name = def.name;
        let mut ok = true;
        if args.len() > def.params.len() {
            self.issue(
                line,
                format!(
                    "{name} takes {} but got {}. Arguments are positional — signature: {}.",
                    plural(def.params.len(), "argument"),
                    args.len(),
                    def.signature()
                ),
            );
            ok = false;
        }
        let mut mapped = Vec::with_capacity(def.params.len());
        for (
            i,
            Param {
                name: param,
                required,
                ty,
            },
        ) in def.params.iter().enumerate()
        {
            let value = args
                .get(i)
                .map(|arg| self.node(arg))
                .filter(|arg| !matches!(arg.expr, Expr::Null));
            match value {
                Some(value) => ok &= self.check(value, ty, name, param),
                None if *required => {
                    let what = if args.get(i).is_some() {
                        "cannot be null"
                    } else {
                        "is missing"
                    };
                    self.issue(
                        line,
                        format!(
                            "{name} required argument `{param}` {what} — signature: {}.",
                            def.signature()
                        ),
                    );
                    ok = false;
                }
                None => {}
            }
            mapped.push(value);
        }
        ok.then_some(mapped)
    }
}
