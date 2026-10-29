//! Shared plugin discovery and brand tiles for both application frontends.
use gpui::{prelude::*, *};
use serde_json::Value;
use zuse_protocol::plugins::PluginDefinition;

pub fn catalog(snapshot: &Value, query: &str, connected_only: bool) -> Vec<PluginDefinition> {
    let fallback: Value =
        serde_json::from_str(include_str!("../../protocol/src/plugin-catalog.json"))
            .expect("bundled catalog");
    let source = snapshot.get("catalog").unwrap_or(&fallback);
    let query = query.trim().to_lowercase();
    let mut items: Vec<PluginDefinition> = source
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| serde_json::from_value::<PluginDefinition>(item.clone()).ok())
        .filter(|item| {
            (!connected_only || connected(snapshot, &item.id))
                && (query.is_empty()
                    || format!(
                        "{} {} {} {} {}",
                        item.name,
                        item.description,
                        item.domain,
                        item.url.as_deref().unwrap_or_default(),
                        item.command.as_deref().unwrap_or_default()
                    )
                    .to_lowercase()
                    .contains(&query))
        })
        .collect();
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(item.id.clone()));
    items
}
pub fn connected(snapshot: &Value, id: &str) -> bool {
    snapshot["connections"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|row| row["pluginId"] == id && row["state"] == "connected")
}
pub fn tile(
    item: &PluginDefinition,
    connected: bool,
    text: Hsla,
    muted: Hsla,
    background: Hsla,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .py(px(8.))
        .px(px(8.))
        .min_w_0()
        .rounded(px(10.))
        .child(logo(&item.domain, &item.name, 36., background, muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text)
                        .truncate()
                        .child(item.name.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(muted)
                        .truncate()
                        .child(item.description.clone()),
                ),
        )
        .child(
            crate::assets::icon(if connected { "check" } else { "plus" })
                .size(px(15.))
                .text_color(muted),
        )
}

pub fn logo(domain: &str, name: &str, extent: f32, background: Hsla, muted: Hsla) -> Div {
    let path = format!("plugin-icons/{domain}");
    div()
        .size(px(extent))
        .flex_none()
        .rounded(px(10.))
        .overflow_hidden()
        .bg(background)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(14.))
        .text_color(muted)
        .child(if zuse_icons::load_plugin_icon(&path).is_some() {
            img(SharedString::from(path))
                .size_full()
                .rounded(px(10.))
                .into_any_element()
        } else {
            div()
                .child(
                    name.chars()
                        .next()
                        .unwrap_or('?')
                        .to_uppercase()
                        .to_string(),
                )
                .into_any_element()
        })
}

/// Choose an unused connection label, retaining disabled accounts in the check.
pub fn next_label(snapshot: &Value, plugin: &PluginDefinition) -> String {
    for index in 1.. {
        let label = if index == 1 {
            plugin.name.clone()
        } else {
            format!("{} {index}", plugin.name)
        };
        let taken = snapshot["connections"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|connection| {
                connection["pluginId"] == plugin.id
                    && connection["label"]
                        .as_str()
                        .is_some_and(|name| name.trim().to_lowercase() == label.to_lowercase())
            });
        if !taken {
            return label;
        }
    }
    unreachable!()
}

/// Catalog groups and limits follow the same rules in both frontends.
pub const PAGE_SIZE: usize = 40;
pub struct Section {
    pub title: &'static str,
    pub key: &'static str,
    pub items: Vec<PluginDefinition>,
}
pub struct Browse {
    pub sections: Vec<Section>,
    pub more: bool,
}
pub fn browse(snapshot: &Value, query: &str, connected_only: bool, page: usize) -> Browse {
    let items = catalog(snapshot, query, connected_only);
    let limit = page.saturating_add(1).saturating_mul(PAGE_SIZE);
    if connected_only || !query.trim().is_empty() {
        let more = items.len() > limit;
        return Browse {
            sections: if items.is_empty() {
                vec![]
            } else {
                vec![Section {
                    title: if connected_only {
                        "Connected"
                    } else {
                        "Results"
                    },
                    key: "plugin-results",
                    items: items.into_iter().take(limit).collect(),
                }]
            },
            more,
        };
    }
    let custom = items
        .iter()
        .filter(|p| p.custom)
        .cloned()
        .collect::<Vec<_>>();
    let featured = items
        .iter()
        .filter(|p| !p.custom && p.featured)
        .take(12)
        .cloned()
        .collect::<Vec<_>>();
    let all = items.into_iter().filter(|p| !p.custom).collect::<Vec<_>>();
    let more = all.len() > limit;
    let sections = [
        ("Custom servers", "plugin-custom", custom),
        ("Featured", "plugin-featured", featured),
        (
            "All plugins",
            "plugin-all",
            all.into_iter().take(limit).collect(),
        ),
    ]
    .into_iter()
    .filter(|(_, _, items)| !items.is_empty())
    .map(|(title, key, items)| Section { title, key, items })
    .collect();
    Browse { sections, more }
}
pub fn connected_count(snapshot: &Value) -> usize {
    snapshot["connections"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["state"] == "connected")
        .filter_map(|row| row["pluginId"].as_str())
        .collect::<std::collections::HashSet<_>>()
        .len()
}
pub fn title(item: &PluginDefinition, text: Hsla, muted: Hsla, background: Hsla) -> Div {
    let subtitle = if item.custom && item.description.trim().is_empty() {
        item.url
            .as_deref()
            .or(item.command.as_deref())
            .unwrap_or(&item.domain)
    } else {
        &item.description
    };
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .line_height(px(16.))
        .gap(px(3.))
        .child(
            div()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text)
                        .child(item.name.clone()),
                )
                .when(item.custom, |v| {
                    v.child(
                        div()
                            .flex_none()
                            .px(px(4.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(background)
                            .text_size(px(10.))
                            .line_height(px(16.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(muted)
                            .child("Custom"),
                    )
                }),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(muted)
                .truncate()
                .child(subtitle.to_owned()),
        )
}
pub fn signed_out<C: 'static>(
    busy: bool,
    action: fn(&mut C, &mut Window, &mut Context<C>),
    cx: &mut Context<C>,
) -> Div {
    div()
        .py(px(32.))
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            crate::settings_style::description_text(
                "Sign in to connect your tools and use them across your agents.",
            )
            .text_size(px(12.)),
        )
        .child(
            crate::settings_style::button("plugin-sign-in", "Sign in")
                .self_start()
                .tab_index(0)
                .when(busy, |v| v.opacity(0.5).cursor_default())
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !busy {
                        action(this, window, cx)
                    }
                })),
        )
}
