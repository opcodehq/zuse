//! Program-level checks between parsing and building: normalization of
//! common formatting variants, name resolution, cycle detection, the
//! expansion bound, and reachability. Nothing here materializes the
//! expanded tree; the bound is computed on the statement graph first.

use std::collections::HashMap;

use crate::catalogue;
use crate::error::Issue;
use crate::syntax::{Expr, MAX_DEPTH, Node, Statement};

/// Nodes the reference-expanded program may visit, summed over statements.
pub(crate) const MAX_VISITED: u64 = 4096;

const BUILTINS: &[&str] = &[
    "Count", "First", "Last", "Sum", "Avg", "Min", "Max", "Sort", "Filter", "Round", "Abs",
    "Floor", "Ceil", "Each",
];
const ACTIONS: &[&str] = &["Action", "Run", "ToAssistant", "OpenUrl", "Set", "Reset"];

/// Statement table after resolution: every reference points at a defined,
/// acyclic statement, and every statement is reachable from `root`.
pub(crate) struct Program {
    pub statements: Vec<Statement>,
    index: HashMap<String, usize>,
}

impl Program {
    /// Follow references until a non-reference node.
    pub(crate) fn deref<'a>(&'a self, mut node: &'a Node) -> &'a Node {
        while let Expr::Ref(name) = &node.expr {
            node = &self.statements[self.index[name]].expr;
        }
        node
    }

    pub(crate) fn root(&self) -> &Node {
        &self.statements[self.index["root"]].expr
    }
}

// ─── Normalization ──────────────────────────────────────────────────────────

/// Recover unambiguous formatting variants on the AST (never on source text,
/// so quoted content is untouched). References stay intact so the regular
/// cycle, reachability and expansion checks still run. Returns whether
/// anything changed.
pub(crate) fn normalize(statements: &mut [Statement]) -> bool {
    let mut changed = false;
    for statement in statements {
        normalize_node(&mut statement.expr, &mut changed);
    }
    changed
}

fn normalize_node(node: &mut Node, changed: &mut bool) {
    match &mut node.expr {
        Expr::Call { name, args } => {
            if name == "CardHeader" && args.len() == 1 {
                *name = "Text".into();
                *changed = true;
            }
            if name == "KeyValue"
                && let Some(Node {
                    expr: Expr::Arr(pairs),
                    ..
                }) = args.first_mut()
            {
                for pair in pairs {
                    normalize_pair(pair, changed);
                }
            }
            for arg in args {
                normalize_node(arg, changed);
            }
        }
        Expr::Arr(items) => {
            for item in items {
                normalize_node(item, changed);
            }
        }
        Expr::Obj(entries) => {
            for (_, value) in entries {
                normalize_node(value, changed);
            }
        }
        _ => {}
    }
}

/// `["k", "v"]` → `{key: "k", value: "v"}`; `{label: …}` → `{key: …}`.
fn normalize_pair(pair: &mut Node, changed: &mut bool) {
    match &mut pair.expr {
        Expr::Arr(items) if items.len() == 2 => {
            let value = items.pop().expect("two items");
            let key = items.pop().expect("two items");
            pair.expr = Expr::Obj(vec![("key".into(), key), ("value".into(), value)]);
            *changed = true;
        }
        Expr::Obj(entries)
            if !entries.iter().any(|(key, _)| key == "key")
                && entries.iter().filter(|(key, _)| key == "label").count() == 1 =>
        {
            for (key, _) in entries.iter_mut() {
                if key == "label" {
                    *key = "key".into();
                }
            }
            *changed = true;
        }
        _ => {}
    }
}

/// Canonical source text: one `name = expr` line per statement.
pub(crate) fn print(statements: &[Statement]) -> String {
    let mut out = String::new();
    for (i, statement) in statements.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&statement.name);
        out.push_str(" = ");
        print_node(&statement.expr, &mut out);
    }
    out
}

fn print_node(node: &Node, out: &mut String) {
    match &node.expr {
        Expr::Str(value) => out.push_str(&json_string(value)),
        Expr::Num(value) => out.push_str(&crate::js_number(*value)),
        Expr::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Expr::Null => out.push_str("null"),
        Expr::Ref(name) => out.push_str(name),
        Expr::Arr(items) => {
            out.push('[');
            print_list(items, out);
            out.push(']');
        }
        Expr::Obj(entries) => {
            out.push('{');
            for (i, (key, value)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&json_string(key));
                out.push_str(": ");
                print_node(value, out);
            }
            out.push('}');
        }
        Expr::Call { name, args } => {
            out.push_str(name);
            out.push('(');
            print_list(args, out);
            out.push(')');
        }
    }
}

fn print_list(items: &[Node], out: &mut String) {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        print_node(item, out);
    }
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings always serialize")
}

// ─── Resolution ─────────────────────────────────────────────────────────────

pub(crate) fn resolve(statements: Vec<Statement>) -> Result<Program, Vec<Issue>> {
    let mut issues = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (i, statement) in statements.iter().enumerate() {
        if let Some(&first) = index.get(&statement.name) {
            issues.push(Issue::at(
                statement.line,
                format!(
                    "Duplicate statement: `{}` is already defined on line {}. Give each statement a unique name.",
                    statement.name, statements[first].line
                ),
            ));
        } else {
            index.insert(statement.name.clone(), i);
        }
    }
    for statement in &statements {
        check_names(&statement.expr, &index, &mut issues);
    }
    if !index.contains_key("root") {
        issues.push(Issue::general(
            "Define root = Component(...); `root` is the required top-level statement.",
        ));
    }
    if !issues.is_empty() {
        return Err(issues);
    }

    let program = Program { statements, index };
    measure(&program)?;
    unreachable(&program)?;
    Ok(program)
}

fn check_names(node: &Node, index: &HashMap<String, usize>, issues: &mut Vec<Issue>) {
    match &node.expr {
        Expr::Ref(name) => {
            if !index.contains_key(name) {
                issues.push(Issue::at(
                    node.line,
                    format!(
                        "Unresolved reference: `{name}` is not defined. Define `{name} = ...` on its own line, inline the value, or quote it if it is text."
                    ),
                ));
            }
        }
        Expr::Call { name, args } => {
            if let Some(message) = call_problem(name, args.len()) {
                issues.push(Issue::at(node.line, message));
            }
            for arg in args {
                check_names(arg, index, issues);
            }
        }
        Expr::Arr(items) => {
            for item in items {
                check_names(item, index, issues);
            }
        }
        Expr::Obj(entries) => {
            for (_, value) in entries {
                check_names(value, index, issues);
            }
        }
        _ => {}
    }
}

fn call_problem(name: &str, arity: usize) -> Option<String> {
    if catalogue::lookup(name).is_some() {
        return None;
    }
    let available = catalogue::advertised_names().collect::<Vec<_>>().join(", ");
    Some(match name {
        "Query" | "Mutation" => "Query() and Mutation() are not supported in UI blocks — they are display-only snapshots. Inline the data as literal values.".to_string(),
        "CardHeader" => format!(
            "CardHeader takes exactly one argument (got {arity}). Prefer Card([children], \"Title\") for a titled section."
        ),
        _ if ACTIONS.contains(&name) => format!(
            "{name}() is an action; UI blocks are display-only and support no actions. Use a Form to collect input."
        ),
        _ if BUILTINS.contains(&name) => format!(
            "{name}() is a runtime builtin, which UI blocks do not support. Compute the value and write it literally."
        ),
        _ => format!("Unknown component {name}. Available: {available}."),
    })
}

/// Expanded size and depth of one statement, memoized.
#[derive(Clone, Copy)]
enum Mark {
    Unvisited,
    Active,
    Done { count: u64, depth: u64 },
}

struct Measure<'a> {
    program: &'a Program,
    marks: Vec<Mark>,
    /// Statements currently being expanded, for cycle paths.
    stack: Vec<usize>,
    cycles: Vec<Issue>,
}

#[derive(Debug)]
struct TooComplex;

/// Reject cycles and enforce the expansion bound — as if every reference
/// were inlined, visiting each statement's expression in turn — without
/// materializing anything. Statement sizes are memoized, so exponential
/// reference fan-out costs linear time here.
fn measure(program: &Program) -> Result<(), Vec<Issue>> {
    let mut state = Measure {
        program,
        marks: vec![Mark::Unvisited; program.statements.len()],
        stack: Vec::new(),
        cycles: Vec::new(),
    };
    let mut visited: u64 = 0;
    let mut deepest: u64 = 0;
    for i in 0..program.statements.len() {
        match state.statement(i) {
            Ok((count, depth)) => {
                visited = visited.saturating_add(count);
                deepest = deepest.max(depth);
            }
            Err(TooComplex) => {
                deepest = u64::MAX;
                break;
            }
        }
    }
    if !state.cycles.is_empty() {
        return Err(state.cycles);
    }
    if visited > MAX_VISITED || deepest > MAX_DEPTH as u64 {
        let detail = if deepest > MAX_DEPTH as u64 {
            format!("references nest more than {MAX_DEPTH} levels deep")
        } else {
            format!("expanding references visits {visited} values (limit {MAX_VISITED})")
        };
        return Err(vec![Issue::general(format!(
            "UI spec is too complex: {detail}. Use fewer components or references — a statement referenced N times is counted N times."
        ))]);
    }
    Ok(())
}

impl Measure<'_> {
    fn statement(&mut self, i: usize) -> Result<(u64, u64), TooComplex> {
        match self.marks[i] {
            Mark::Done { count, depth } => return Ok((count, depth)),
            Mark::Active => {
                let start = self.stack.iter().position(|&s| s == i).unwrap_or(0);
                let mut path: Vec<&str> = self.stack[start..]
                    .iter()
                    .map(|&s| self.program.statements[s].name.as_str())
                    .collect();
                path.push(&self.program.statements[i].name);
                let line = self.program.statements[*self.stack.last().unwrap_or(&i)].line;
                self.cycles.push(Issue::at(
                    line,
                    format!(
                        "Cyclic reference: {}. A component cannot contain itself; remove one of these references.",
                        path.join(" → ")
                    ),
                ));
                // Count it as a leaf so the rest of the graph is still checked.
                return Ok((0, 0));
            }
            Mark::Unvisited => {}
        }
        // Every reference hop adds at least one level, so a longer chain
        // can never fit and recursion stays shallow on hostile input.
        if self.stack.len() > MAX_DEPTH {
            return Err(TooComplex);
        }
        self.marks[i] = Mark::Active;
        self.stack.push(i);
        let program = self.program;
        let result = self.node(&program.statements[i].expr);
        self.stack.pop();
        let (count, depth) = result?;
        self.marks[i] = Mark::Done { count, depth };
        Ok((count, depth))
    }

    /// `(visited nodes, max depth below this node)` with references expanded.
    fn node(&mut self, node: &Node) -> Result<(u64, u64), TooComplex> {
        let children: Box<dyn Iterator<Item = &Node>> = match &node.expr {
            Expr::Ref(name) => {
                let (count, depth) = self.statement(self.program.index[name])?;
                return Ok((count.saturating_add(1), depth.saturating_add(1)));
            }
            Expr::Arr(items) | Expr::Call { args: items, .. } => Box::new(items.iter()),
            Expr::Obj(entries) => Box::new(entries.iter().map(|(_, value)| value)),
            _ => return Ok((1, 0)),
        };
        let mut count: u64 = 1;
        let mut depth: u64 = 0;
        for child in children {
            let (c, d) = self.node(child)?;
            count = count.saturating_add(c);
            depth = depth.max(d.saturating_add(1));
        }
        Ok((count, depth))
    }
}

fn unreachable(program: &Program) -> Result<(), Vec<Issue>> {
    let mut reached = vec![false; program.statements.len()];
    let mut pending = vec![program.index["root"]];
    reached[pending[0]] = true;
    while let Some(i) = pending.pop() {
        collect_refs(&program.statements[i].expr, &mut |name| {
            let target = program.index[name];
            if !reached[target] {
                reached[target] = true;
                pending.push(target);
            }
        });
    }
    let unused: Vec<String> = program
        .statements
        .iter()
        .zip(&reached)
        .filter(|(_, reached)| !**reached)
        .map(|(statement, _)| format!("`{}` (line {})", statement.name, statement.line))
        .collect();
    if unused.is_empty() {
        return Ok(());
    }
    Err(vec![Issue::general(format!(
        "Unused statements: {}. Every statement must be reachable from root — reference it from root's tree or delete it.",
        unused.join(", ")
    ))])
}

fn collect_refs<'a>(node: &'a Node, found: &mut impl FnMut(&'a str)) {
    match &node.expr {
        Expr::Ref(name) => found(name),
        Expr::Arr(items) | Expr::Call { args: items, .. } => {
            for item in items {
                collect_refs(item, found);
            }
        }
        Expr::Obj(entries) => {
            for (_, value) in entries {
                collect_refs(value, found);
            }
        }
        _ => {}
    }
}
