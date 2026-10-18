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
                    || format!("{} {}", item.name, item.description)
                        .to_lowercase()
                        .contains(&query))
        })
        .collect();
    items.sort_by_key(|item| (!item.featured, item.name.to_lowercase()));
    items.dedup_by(|a, b| a.id == b.id);
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
    let path = format!("plugin-icons/{}", item.domain);
    let brand = div()
        .size(px(36.))
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
                .child(item.name.chars().next().unwrap_or('?').to_string())
                .into_any_element()
        });
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .py(px(8.))
        .px(px(6.))
        .min_w_0()
        .rounded(px(8.))
        .child(brand)
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
                        .text_color(text)
                        .truncate()
                        .child(item.name.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.))
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
