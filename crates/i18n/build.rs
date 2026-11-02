use fluent_syntax::{ast, parser};
use std::{collections::BTreeSet, env, fs, path::PathBuf};

fn variables(pattern: &ast::Pattern<&str>, vars: &mut BTreeSet<String>) {
    for element in &pattern.elements {
        if let ast::PatternElement::Placeable { expression } = element {
            expression_vars(expression, vars);
        }
    }
}
fn expression_vars(expression: &ast::Expression<&str>, vars: &mut BTreeSet<String>) {
    match expression {
        ast::Expression::Inline(inline) => inline_vars(inline, vars),
        ast::Expression::Select { selector, variants } => {
            inline_vars(selector, vars);
            for variant in variants {
                variables(&variant.value, vars);
            }
        }
    }
}
fn inline_vars(inline: &ast::InlineExpression<&str>, vars: &mut BTreeSet<String>) {
    match inline {
        ast::InlineExpression::VariableReference { id } => {
            vars.insert(id.name.into());
        }
        ast::InlineExpression::Placeable { expression } => expression_vars(expression, vars),
        ast::InlineExpression::FunctionReference { arguments, .. } => {
            for arg in &arguments.positional {
                inline_vars(arg, vars);
            }
            for arg in &arguments.named {
                inline_vars(&arg.value, vars);
            }
        }
        _ => {}
    }
}
fn identifier(name: &str) -> String {
    let value = name.replace('-', "_");
    match value.as_str() {
        "type" | "match" | "ref" | "self" | "mod" | "fn" | "loop" | "move" | "use" | "super"
        | "crate" | "async" | "await" | "in" | "continue" | "gen" | "where" | "for" | "return"
        | "const" | "static" | "break" | "trait" | "impl" | "let" | "pub" | "unsafe" | "extern"
        | "enum" | "struct" | "if" | "else" | "true" | "false" | "dyn" | "as" | "mut" | "while" => {
            format!("{value}_")
        }
        _ => value,
    }
}
fn main() {
    println!("cargo:rerun-if-changed=locales");
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let mut namespaces: Vec<_> = fs::read_dir(root.join("locales/en"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "ftl"))
        .collect();
    namespaces.sort();
    let mut generated = String::from("// Generated from English Fluent resources.\n");
    let mut resources = String::from(
        "pub(crate) fn resources(locale: Locale) -> &'static [&'static str] { match locale {\n",
    );
    for (variant, locale) in [
        ("En", "en"),
        ("Fr", "fr"),
        ("De", "de"),
        ("ZhHans", "zh-Hans"),
        ("ZhHant", "zh-Hant"),
        ("Ja", "ja"),
        ("Ko", "ko"),
    ] {
        resources.push_str(&format!("Locale::{variant} => &[\n"));
        for path in &namespaces {
            let filename = path.file_name().unwrap().to_str().unwrap();
            let file = root.join("locales").join(locale).join(filename);
            let text = fs::read_to_string(&file).unwrap();
            parser::parse(text.as_str())
                .unwrap_or_else(|(_, errors)| panic!("{}: {errors:?}", file.display()));
            resources.push_str(&format!("include_str!({:?}),\n", file));
        }
        resources.push_str("],\n");
    }
    resources.push_str("} }\n");
    let mut pseudo_resources = String::from(
        "#[cfg(debug_assertions)] pub(crate) fn pseudo_resources() -> &'static [&'static str] { &[\n",
    );
    for path in &namespaces {
        let mut ast = parser::parse(fs::read_to_string(path).unwrap()).unwrap();
        for entry in &mut ast.body {
            if let ast::Entry::Message(message) = entry {
                if let Some(value) = &mut message.value {
                    expand_pattern(value);
                }
                for attribute in &mut message.attributes {
                    expand_pattern(&mut attribute.value);
                }
            }
        }
        let pseudo_file = PathBuf::from(env::var("OUT_DIR").unwrap()).join(format!(
            "pseudo-{}",
            path.file_name().unwrap().to_str().unwrap()
        ));
        fs::write(&pseudo_file, fluent_syntax::serializer::serialize(&ast)).unwrap();
        pseudo_resources.push_str(&format!("include_str!({:?}),\n", pseudo_file));
    }
    pseudo_resources.push_str("] }\n");
    resources.push_str(&pseudo_resources);
    for path in &namespaces {
        let namespace = path.file_stem().unwrap().to_str().unwrap();
        let text = fs::read_to_string(path).unwrap();
        let ast = parser::parse(text.as_str()).unwrap();
        generated.push_str(&format!(
            "#[allow(non_snake_case)] pub mod {} {{\n",
            identifier(namespace)
        ));
        for entry in ast.body {
            if let ast::Entry::Message(message) = entry {
                let id = message.id.name;
                let name = identifier(id.strip_prefix(&format!("{namespace}-")).unwrap());
                if let Some(pattern) = message.value {
                    accessor(&mut generated, id, &name, &pattern, None);
                }
                for attribute in message.attributes {
                    let accessor_name = format!("{name}_{}", identifier(attribute.id.name));
                    accessor(
                        &mut generated,
                        id,
                        &accessor_name,
                        &attribute.value,
                        Some(attribute.id.name),
                    );
                }
            }
        }
        generated.push_str("}\n");
    }
    let output = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(output.join("messages.rs"), generated).unwrap();
    fs::write(output.join("resources.rs"), resources).unwrap();
}

fn expand_text(value: &str) -> String {
    let mut markup = false;
    let mut output = String::new();
    for c in value.chars() {
        if c == '<' {
            markup = true;
        }
        if markup {
            output.push(c);
        } else {
            let replacement = match c {
                'a' => "áá",
                'e' => "éé",
                'i' => "íí",
                'o' => "óó",
                'u' => "úú",
                'A' => "ÁÁ",
                'E' => "ÉÉ",
                'I' => "ÍÍ",
                'O' => "ÓÓ",
                'U' => "ÚÚ",
                _ => {
                    output.push(c);
                    continue;
                }
            };
            output.push_str(replacement);
        }
        if c == '>' {
            markup = false;
        }
    }
    output
}
fn expand_pattern(pattern: &mut ast::Pattern<String>) {
    for element in &mut pattern.elements {
        match element {
            ast::PatternElement::TextElement { value } => *value = expand_text(value),
            ast::PatternElement::Placeable { expression } => {
                if let ast::Expression::Select { variants, .. } = expression {
                    for variant in variants {
                        expand_pattern(&mut variant.value);
                    }
                }
            }
        }
    }
}

fn accessor(
    generated: &mut String,
    id: &str,
    name: &str,
    pattern: &ast::Pattern<&str>,
    attribute: Option<&str>,
) {
    let mut vars = BTreeSet::new();
    variables(pattern, &mut vars);
    let parameters = vars
        .iter()
        .map(|v| {
            format!(
                ", {}: impl Into<fluent_bundle::FluentValue<'a>>",
                identifier(v)
            )
        })
        .collect::<String>();
    generated.push_str(&format!(
        "pub fn {name}<'a>(l10n: &crate::Localizer{parameters}) -> String {{\n"
    ));
    if !vars.is_empty() {
        generated.push_str("let mut args = fluent_bundle::FluentArgs::new();\n");
        for var in &vars {
            generated.push_str(&format!("args.set({var:?}, {});\n", identifier(var)));
        }
    }
    let args = if vars.is_empty() {
        "None"
    } else {
        "Some(&args)"
    };
    if let Some(attribute) = attribute {
        generated.push_str(&format!("l10n.attribute({id:?},{attribute:?},{args})\n"));
    } else {
        generated.push_str(&format!("l10n.format({id:?},{args})\n"));
    }
    generated.push_str("}\n");
}
