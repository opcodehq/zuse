use super::*;

#[test]
fn locale_negotiation_respects_script_then_region_and_preference_order() {
    for (input, expected) in [
        ("zh-Hans-TW", Locale::ZhHans),
        ("zh-Hant-CN", Locale::ZhHant),
        ("zh_TW.UTF-8", Locale::ZhHant),
        ("zh-HK", Locale::ZhHant),
        ("zh-MO", Locale::ZhHant),
        ("zh-SG", Locale::ZhHans),
        ("zh", Locale::ZhHans),
        ("fr-CA", Locale::Fr),
    ] {
        assert_eq!(Locale::from_language(input), Some(expected), "{input}");
    }
    assert_eq!(
        LanguagePreference::System.resolve(&["xx".into(), "ko-KR".into()]),
        Locale::Ko
    );
    assert_eq!(
        LanguagePreference::Explicit(Locale::De).resolve(&["fr".into()]),
        Locale::De
    );
    assert_eq!(
        LanguagePreference::System.resolve(&["bad!".into()]),
        Locale::En
    );
}
#[test]
fn invalid_saved_language_does_not_discard_other_preferences() {
    for malformed in ["null", "123", "{}", "[]"] {
        assert_eq!(
            serde_json::from_str::<LanguagePreference>(malformed).unwrap(),
            LanguagePreference::System
        );
    }
    assert_eq!(
        serde_json::from_str::<LanguagePreference>("\"unsupported\"").unwrap(),
        LanguagePreference::System
    );
    assert_eq!(
        serde_json::to_string(&LanguagePreference::Explicit(Locale::ZhHant)).unwrap(),
        "\"zh-Hant\""
    );
}
#[test]
fn independent_contexts_do_not_share_language_and_typed_arguments_work() {
    let chinese = Localizer::new(Locale::ZhHans);
    let french = Localizer::new(Locale::Fr);
    assert_eq!(messages::common::save(&chinese), "保存");
    assert_eq!(messages::common::save(&french), "Enregistrer");
    let rendered = messages::common::show_more_count(&chinese, 3);
    assert!(rendered.contains('3'));
    assert!(!rendered.contains("show_more"));
}
#[test]
fn formatting_is_locale_aware_and_rejects_invalid_dates() {
    let de = Localizer::new(Locale::De);
    assert_eq!(de.number(1234567), "1.234.567");
    assert!(de.date(2026, 11, 2).unwrap().contains("2026"));
    assert_eq!(de.date(2026, 2, 30), None);
}
#[test]
fn contributor_corrections_are_preserved() {
    let hans = Localizer::new(Locale::ZhHans);
    let hant = Localizer::new(Locale::ZhHant);
    assert_eq!(hans.text("settings-cloud_machines_pane_checkout"), "结账");
    assert_eq!(hant.text("settings-cloud_machines_pane_checkout"), "結帳");
    assert_eq!(
        format!(
            "3 {}{}",
            hans.text("chat-turn_summary_tool"),
            hans.text("chat-turn_summary_call")
        ),
        "3 次工具调用"
    );
    assert!(
        hans.text("projects-worktree_setup_card_branching_a_fresh_worktree")
            .contains("工作树")
    );
    assert!(
        hant.text("projects-pr_pane_all_checks_passed")
            .contains("通過")
    );
    assert!(
        !hant
            .text("projects-pr_pane_all_checks_passed")
            .contains("透過")
    );
}

#[test]
#[ignore = "one-time audit against the pinned import; intentional later translation edits may differ"]
fn every_imported_message_preserves_text_placeholders_and_markup() {
    use sha2::{Digest, Sha256};
    let report: serde_json::Value =
        serde_json::from_str(include_str!("../migration.json")).unwrap();
    for locale in Locale::ALL {
        let l10n = Localizer::new(locale);
        for (key, record) in report["messages"].as_object().unwrap() {
            let mut args = Arguments::new();
            for variable in record["variables"].as_array().unwrap() {
                let variable = variable.as_str().unwrap();
                args.set(variable, format!("{{{{{variable}}}}}"));
            }
            let rendered = l10n
                .format(record["target"].as_str().unwrap(), Some(&args))
                .replace(['\u{2068}', '\u{2069}'], "");
            let actual = format!("{:x}", Sha256::digest(rendered.as_bytes()));
            let expected = record["translations"][locale.code()].as_str().unwrap();
            assert_eq!(actual, expected, "{locale:?}: {key}: {rendered}");
        }
    }
}

#[test]
fn all_catalogs_resolve_and_translation_variables_are_supplied_by_english() {
    use fluent_syntax::{ast, parser};
    use std::collections::{BTreeMap, BTreeSet};
    fn walk(pattern: &ast::Pattern<&str>, variables: &mut BTreeSet<String>) {
        for element in &pattern.elements {
            if let ast::PatternElement::Placeable { expression } = element {
                expr(expression, variables);
            }
        }
    }
    fn expr(value: &ast::Expression<&str>, variables: &mut BTreeSet<String>) {
        match value {
            ast::Expression::Inline(value) => inline(value, variables),
            ast::Expression::Select { selector, variants } => {
                inline(selector, variables);
                assert!(
                    variants.iter().any(|v| v.default),
                    "selector must have a fallback"
                );
                for variant in variants {
                    walk(&variant.value, variables);
                }
            }
        }
    }
    fn inline(value: &ast::InlineExpression<&str>, variables: &mut BTreeSet<String>) {
        match value {
            ast::InlineExpression::VariableReference { id } => {
                variables.insert(id.name.into());
            }
            ast::InlineExpression::Placeable { expression } => expr(expression, variables),
            ast::InlineExpression::FunctionReference { arguments, .. } => {
                for arg in &arguments.positional {
                    inline(arg, variables);
                }
                for arg in &arguments.named {
                    inline(&arg.value, variables);
                }
            }
            _ => {}
        }
    }
    fn contracts(locale: Locale) -> BTreeMap<String, BTreeSet<String>> {
        let mut contracts = BTreeMap::new();
        for resource in resources(locale) {
            for entry in parser::parse(*resource).unwrap().body {
                if let ast::Entry::Message(message) = entry {
                    let mut vars = BTreeSet::new();
                    if let Some(pattern) = message.value {
                        walk(&pattern, &mut vars);
                    }
                    for attribute in message.attributes {
                        walk(&attribute.value, &mut vars);
                    }
                    assert!(
                        contracts.insert(message.id.name.into(), vars).is_none(),
                        "duplicate key"
                    );
                }
            }
        }
        contracts
    }
    let english = contracts(Locale::En);
    for locale in Locale::ALL {
        let translated = contracts(locale);
        assert!(
            english.keys().all(|key| translated.contains_key(key)),
            "{locale:?}: missing English key"
        );
        let l10n = Localizer::new(locale);
        for (id, vars) in translated {
            let source = english
                .get(&id)
                .or_else(|| {
                    id.strip_suffix("_many")
                        .and_then(|base| english.get(&format!("{base}_other")))
                })
                .expect(
                    "translation key needs an English source or a locale-specific plural variant",
                );
            assert!(vars.is_subset(source), "{locale:?}: {id}: unknown variable");
            let mut args = Arguments::new();
            for var in source {
                args.set(var.as_str(), 3);
            }
            let message = l10n.bundle.get_message(&id).unwrap();
            for pattern in message
                .value()
                .into_iter()
                .chain(message.attributes().map(|a| a.value()))
            {
                let mut errors = Vec::new();
                let _ = l10n
                    .bundle
                    .format_pattern(pattern, Some(&args), &mut errors);
                assert!(errors.is_empty(), "{locale:?}: {id}: {errors:?}");
            }
        }
    }
}

#[test]
fn locale_aware_plurals_and_attributes_use_the_same_context() {
    let en = Localizer::new(Locale::En);
    let zh = Localizer::new(Locale::ZhHans);
    assert_eq!(
        messages::simulators::summary_devices_plural(&en, 1).replace(['\u{2068}', '\u{2069}'], ""),
        "1 device"
    );
    assert_eq!(
        messages::simulators::summary_devices_plural(&en, 2).replace(['\u{2068}', '\u{2069}'], ""),
        "2 devices"
    );
    assert_eq!(messages::native::language_settings_aria_label(&zh), "语言");
    assert_eq!(messages::native::about_zuse(&zh), "关于 Zuse");
}
#[test]
fn expanded_english_keeps_interpolated_values_intact() {
    let expanded = Localizer::expanded();
    let value = messages::common::collapse(&expanded, "用户/project/code.rs");
    assert!(value.contains("用户/project/code.rs"));
    assert!(
        value.len()
            > messages::common::collapse(&Localizer::default(), "用户/project/code.rs").len()
    );
}
#[test]
fn missing_or_broken_translations_fall_back_to_english() {
    let mut broken = Bundle::new_concurrent(vec!["fr".parse().unwrap()]);
    broken
        .add_resource(FluentResource::try_new("common-save = { $missing }\n".into()).unwrap())
        .unwrap();
    let mut l10n = Localizer::new(Locale::Fr);
    l10n.bundle = Arc::new(broken);
    assert_eq!(messages::common::save(&l10n), "Save");
    assert_eq!(messages::common::cancel(&l10n), "Cancel");
    assert_eq!(l10n.text("nonexistent-key"), "nonexistent-key");
}
