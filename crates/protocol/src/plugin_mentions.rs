//! Plugin chips: `@Linear` in a composer. The draft and transcript keep the
//! link; the agent receives a plain instruction to use that connected plugin
//! through Zuse's `plugins_search`/`plugins_call` tools.
use std::ops::Range;

use crate::file_mentions::escape_mention_label;

pub const PLUGIN_MENTION_SCHEME: &str = "zuse-plugin:";

pub struct PluginMentionLink {
    pub range: Range<usize>,
    pub name: String,
    pub plugin_id: String,
}

/// Catalog and custom-server ids are plain slugs: never a path or markup.
fn valid_plugin_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && !id.contains("..")
}

fn clean_name(name: &str) -> String {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        "Plugin".into()
    } else {
        name
    }
}

/// `[name](zuse-plugin:<id>)`, or `None` for an id that isn't a plain slug.
pub fn plugin_mention_link(plugin_id: &str, name: &str) -> Option<String> {
    valid_plugin_id(plugin_id).then(|| {
        format!(
            "[{}]({PLUGIN_MENTION_SCHEME}{plugin_id})",
            escape_mention_label(&clean_name(name))
        )
    })
}

/// Only decode canonical chip links, never escaped text or code examples.
pub fn plugin_mention_links(text: &str) -> Vec<PluginMentionLink> {
    if !text.contains(PLUGIN_MENTION_SCHEME) {
        return Vec::new();
    }
    let mut image_depth = 0;
    let mut open: Option<(Range<usize>, String)> = None;
    let mut name = String::new();
    let mut links = Vec::new();
    for (event, range) in pulldown_cmark::Parser::new(text).into_offset_iter() {
        match &event {
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::Image { .. }) => image_depth += 1,
            pulldown_cmark::Event::End(pulldown_cmark::TagEnd::Image) => image_depth -= 1,
            _ => {}
        }
        if image_depth > 0 {
            continue;
        }
        match event {
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link { dest_url, .. }) => {
                if let Some(id) = dest_url.strip_prefix(PLUGIN_MENTION_SCHEME)
                    && valid_plugin_id(id)
                {
                    open = Some((range, id.to_owned()));
                    name.clear();
                }
            }
            pulldown_cmark::Event::Text(text) | pulldown_cmark::Event::Code(text)
                if open.is_some() =>
            {
                name.push_str(&text);
            }
            pulldown_cmark::Event::End(pulldown_cmark::TagEnd::Link) => {
                if let Some((range, plugin_id)) = open.take()
                    && plugin_mention_link(&plugin_id, &name).as_deref()
                        == Some(&text[range.clone()])
                {
                    links.push(PluginMentionLink {
                        range,
                        name: clean_name(&name),
                        plugin_id,
                    });
                }
            }
            _ => {}
        }
    }
    links
}

/// Replace each plugin chip with text any agent can act on. The durable
/// transcript keeps the chip; only outgoing text changes.
pub fn plugin_mention_prompt(text: &str) -> String {
    let mut out = String::new();
    let mut at = 0;
    for link in plugin_mention_links(text) {
        out.push_str(&text[at..link.range.start]);
        out.push_str(&format!(
            "@{} (connected Zuse plugin id {}; find its tools with the Zuse plugins_search \
             tool and run them with plugins_call)",
            link.name, link.plugin_id
        ));
        at = link.range.end;
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chips_round_trip_and_reach_the_agent_as_an_instruction() {
        let link = plugin_mention_link("linear", "Linear").unwrap();
        assert_eq!(link, "[Linear](zuse-plugin:linear)");
        let text = format!("Triage my {link} issues");
        let links = plugin_mention_links(&text);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].plugin_id, "linear");
        assert_eq!(&text[links[0].range.clone()], link);
        assert_eq!(
            plugin_mention_prompt(&text),
            "Triage my @Linear (connected Zuse plugin id linear; find its tools with the Zuse \
             plugins_search tool and run them with plugins_call) issues"
        );
    }

    #[test]
    fn only_canonical_plain_ids_become_chips() {
        assert!(plugin_mention_link("../x", "t").is_none());
        assert!(plugin_mention_link("", "t").is_none());
        let link = plugin_mention_link("custom-ab12", "Local Echo").unwrap();
        for literal in [
            format!("`{link}`"),
            format!("\\{link}"),
            "[x](zuse-plugin:a b)".to_owned(),
            "[x](zuse-plugin:a/b)".to_owned(),
        ] {
            assert!(plugin_mention_links(&literal).is_empty(), "{literal}");
            assert_eq!(plugin_mention_prompt(&literal), literal);
        }
    }
}
