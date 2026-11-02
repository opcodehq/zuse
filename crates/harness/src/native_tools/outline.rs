//! Fresh structural outlines. No background index, stale-result cache or LSP process.
use super::{
    error,
    files::{CheckoutFiles, check},
};
use crate::Error;
use serde_json::{Value, json};
use std::path::Path;
use tokio_util::sync::CancellationToken;
pub(super) fn outline(
    files: &CheckoutFiles,
    path: &Path,
    limit: usize,
    cancel: &CancellationToken,
) -> Result<Value, Error> {
    let bytes = files.read(path, true)?;
    let source = std::str::from_utf8(&bytes).map_err(|_| error("Outline requires UTF-8 text"))?;
    let (language, name) = match path.extension().and_then(|s| s.to_str()) {
        Some("rs") => (tree_sitter_rust::LANGUAGE.into(), "rust"),
        Some("py" | "pyi") => (tree_sitter_python::LANGUAGE.into(), "python"),
        Some("tsx" | "jsx") => (tree_sitter_typescript::LANGUAGE_TSX.into(), "tsx"),
        Some("ts" | "js" | "mjs" | "cjs") => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "typescript",
        ),
        _ => {
            return Err(error(
                "Structural outline supports Rust, Python, JavaScript and TypeScript; use grep for other languages",
            ));
        }
    };
    check(cancel)?;
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&language)
        .map_err(|_| error("Outline grammar unavailable"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| error("Outline parse failed"))?;
    check(cancel)?;
    let mut cursor = tree.walk();
    let mut symbols = vec![];
    let mut visited = 0usize;
    let mut truncated = false;
    loop {
        visited += 1;
        if visited.is_multiple_of(256) {
            check(cancel)?;
        }
        if visited > 200000 {
            truncated = true;
            break;
        }
        let node = cursor.node();
        let kind = node.kind();
        let selected = matches!(
            kind,
            "function_item"
                | "struct_item"
                | "enum_item"
                | "trait_item"
                | "impl_item"
                | "mod_item"
                | "type_item"
                | "function_definition"
                | "class_definition"
                | "function_declaration"
                | "class_declaration"
                | "method_definition"
                | "interface_declaration"
                | "type_alias_declaration"
                | "enum_declaration"
        ) || (kind == "variable_declarator"
            && node
                .child_by_field_name("value")
                .is_some_and(|v| matches!(v.kind(), "arrow_function" | "function_expression")));
        if selected {
            if symbols.len() >= limit {
                truncated = true;
                break;
            }
            let label = node
                .child_by_field_name("name")
                .or_else(|| node.child_by_field_name("type"))
                .and_then(|n| n.utf8_text(&bytes).ok())
                .unwrap_or(kind);
            let signature = node
                .utf8_text(&bytes)
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(240)
                .collect::<String>();
            symbols.push(json!({"name":label.chars().take(200).collect::<String>(),"kind":kind,"start_line":node.start_position().row+1,"end_line":node.end_position().row+1,"signature":signature}));
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return Ok(
                    json!({"path":path,"hash":super::files::digest(&bytes),"language":name,"symbols":symbols,"truncated":truncated,"has_parse_errors":tree.root_node().has_error()}),
                );
            }
        }
    }
    Ok(
        json!({"path":path,"hash":super::files::digest(&bytes),"language":name,"symbols":symbols,"truncated":truncated,"has_parse_errors":tree.root_node().has_error()}),
    )
}
