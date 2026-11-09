//! Compact site links: the favicon and short label a web URL shows as in the
//! composer, sent messages and replies. Only the presentation changes; the URL
//! itself stays the text that is copied, sent and opened.
use gpui::{App, Bounds, Hsla, Pixels, RenderImage, SharedString, Window};
use std::{ops::Range, sync::Arc};

/// Characters that end a bare URL in prose.
fn ends_url(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\'' | '`')
}

/// Every bare `http(s)://` URL in `text` with its byte range. Trailing
/// sentence punctuation stays outside the link, and a URL glued to a word
/// (`xhttps://…`) is not one.
pub(crate) fn site_links_in_text(text: &str) -> Vec<Range<usize>> {
    let mut links = Vec::new();
    let bytes = text.as_bytes();
    let mut at = 0;
    while let Some(found) = text[at..].find(['h', 'H']) {
        let start = at + found;
        at = start + 1;
        let rest = &text[start..];
        let scheme = if rest
            .as_bytes()
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"https://"))
        {
            8
        } else if rest
            .as_bytes()
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"http://"))
        {
            7
        } else {
            continue;
        };
        if start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
            continue;
        }
        let len = rest.find(ends_url).unwrap_or(rest.len());
        let url = rest[..len].trim_end_matches(['.', ',', ';', ':', '!', '?', ']', ')', '}']);
        if url.len() > scheme && host(url).is_some() {
            links.push(start..start + url.len());
            at = start + url.len();
        }
    }
    links
}

/// The lowercase host of an `http(s)` URL.
pub(crate) fn host(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed
        .host_str()
        .filter(|host| !host.is_empty())
        .map(str::to_ascii_lowercase)
}

/// Sites whose links get a mark and a label of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KnownSite {
    GitHub,
    GitLab,
    Linear,
    Figma,
    Notion,
}

/// Percent-decode one path segment, keeping the raw text when it is not UTF-8.
fn decode(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = segment
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| segment.to_string())
}

/// The mark and short label of a link to a known site: `owner/repo#123` for
/// a GitHub pull request or issue, a Linear issue id, a Figma file's name, a
/// Notion page title.
pub(crate) fn known_site_link(url: &str) -> Option<(KnownSite, String)> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.port().is_some() {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let parts: Vec<String> = parsed
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .map(decode)
        .collect();
    let joined = || parts.join("/");
    let numeric = |id: &str| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit());
    match host {
        "github.com" => {
            let repository = parts.iter().take(2).cloned().collect::<Vec<_>>().join("/");
            let kind = parts.get(2).map(String::as_str);
            let id = parts.get(3).map(String::as_str);
            let label = match (parts.len() >= 2, kind, id) {
                (true, Some("pull" | "issues" | "discussions"), Some(id)) if numeric(id) => {
                    format!("{repository}#{id}")
                }
                (true, Some("commit"), Some(id)) => {
                    format!("{repository}@{}", id.chars().take(7).collect::<String>())
                }
                _ if parts.is_empty() => "GitHub".into(),
                _ => joined(),
            };
            Some((KnownSite::GitHub, label))
        }
        "gitlab.com" => {
            let separator = parts.iter().position(|part| part == "-");
            let label = separator
                .filter(|separator| *separator > 0)
                .and_then(|separator| {
                    let kind = parts.get(separator + 1)?;
                    let id = parts.get(separator + 2).filter(|id| numeric(id))?;
                    let mark = match kind.as_str() {
                        "merge_requests" => "!",
                        "issues" => "#",
                        _ => return None,
                    };
                    Some(format!("{}{mark}{id}", parts[..separator].join("/")))
                })
                .unwrap_or_else(|| {
                    if parts.is_empty() {
                        "GitLab".into()
                    } else {
                        joined()
                    }
                });
            Some((KnownSite::GitLab, label))
        }
        "linear.app" => {
            let issue = parts.iter().position(|part| part == "issue");
            let label = issue
                .and_then(|issue| parts.get(issue + 1))
                .or(parts.last())
                .cloned()
                .unwrap_or_else(|| "Linear".into());
            Some((KnownSite::Linear, label))
        }
        "figma.com" => {
            let label = parts
                .get(2)
                .map(|name| name.replace('-', " "))
                .unwrap_or_else(|| "Figma".into());
            Some((KnownSite::Figma, label))
        }
        _ if host == "notion.so" || host == "notion.site" || host.ends_with(".notion.site") => {
            let title = parts.last().map(|last| {
                // Page slugs end in a 32-digit hex id.
                let bytes = last.as_bytes();
                let id_start = last.len().saturating_sub(32);
                let tail_is_id = last.len() >= 32
                    && last.is_char_boundary(id_start)
                    && bytes[id_start..].iter().all(u8::is_ascii_hexdigit);
                let stem = if tail_is_id { &last[..id_start] } else { last };
                stem.trim_end_matches('-').replace('-', " ")
            });
            let label = title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| "Notion".into());
            Some((KnownSite::Notion, label))
        }
        _ => None,
    }
}

/// The longest label a link shows before it is cut with an ellipsis.
const MAX_LABEL_CHARS: usize = 48;

/// The short label a link shows: a known site's own label, otherwise its host
/// and path (no scheme, `www.`, query or fragment).
pub(crate) fn link_label(url: &str) -> String {
    let label = known_site_link(url)
        .map(|(_, label)| label)
        .or_else(|| {
            let parsed = url::Url::parse(url).ok()?;
            let host = parsed.host_str()?.to_ascii_lowercase();
            let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
            let path = decode(parsed.path().trim_end_matches('/'));
            let port = parsed
                .port()
                .map(|port| format!(":{port}"))
                .unwrap_or_default();
            Some(format!("{host}{port}{path}"))
        })
        .unwrap_or_else(|| url.to_string());
    if label.chars().count() > MAX_LABEL_CHARS {
        let mut cut: String = label.chars().take(MAX_LABEL_CHARS - 1).collect();
        cut.push('…');
        cut
    } else {
        label
    }
}

/// Where a link's favicon comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Favicon {
    /// A bundled mark, drawn in the link's tint.
    Mark(&'static str),
    /// The site's own `/favicon.ico`, falling back to the globe.
    Remote(SharedString),
    /// No reachable icon: the globe.
    Globe,
}

/// The favicon of `url`. Known sites use bundled marks; other public hosts
/// load `https://<host>/favicon.ico` through the app's public-HTTPS asset
/// client; anything else (local hosts, private addresses) shows the globe.
pub(crate) fn favicon(url: &str) -> Favicon {
    match known_site_link(url).map(|(site, _)| site) {
        Some(KnownSite::GitHub) => return Favicon::Mark(crate::icons::SITE_GITHUB),
        Some(KnownSite::GitLab) => return Favicon::Mark(crate::icons::SITE_GITLAB),
        Some(KnownSite::Linear) => return Favicon::Mark(crate::icons::SITE_LINEAR),
        Some(KnownSite::Figma) => return Favicon::Mark(crate::icons::SITE_FIGMA),
        Some(KnownSite::Notion) => return Favicon::Mark(crate::icons::SITE_NOTION),
        None => {}
    }
    let Some(host) = host(url) else {
        return Favicon::Globe;
    };
    match url::Url::parse(&format!("https://{host}/favicon.ico")) {
        Ok(icon) if crate::asset_http::public_https(&icon) => {
            Favicon::Remote(SharedString::from(icon.to_string()))
        }
        _ => Favicon::Globe,
    }
}

/// A remote favicon once it has loaded. GPUI's asset cache keys the load by
/// its URI, so each host is fetched once per run, failures included; until
/// then (and after a failure) this is `None` and callers draw the globe, so
/// a slow site never holds up a frame.
fn remote_favicon(
    uri: &SharedString,
    window: &mut Window,
    cx: &mut App,
) -> Option<Arc<RenderImage>> {
    window
        .use_asset::<gpui::ImageAssetLoader>(&gpui::Resource::Uri(uri.clone().into()), cx)
        .and_then(Result::ok)
}

/// Paint `url`'s favicon into `bounds` (the 1em icon box). Marks and the
/// globe take `tint`; a site's own icon keeps its colours, at radius 2.
pub(crate) fn paint_favicon(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    url: &str,
    tint: Hsla,
    cx: &mut App,
) {
    let mark = match favicon(url) {
        Favicon::Mark(path) => path,
        Favicon::Remote(uri) => match remote_favicon(&uri, window, cx) {
            Some(image) => {
                let _ =
                    window.paint_image(bounds, gpui::Corners::all(gpui::px(2.0)), image, 0, false);
                return;
            }
            None => crate::icons::GLOBE,
        },
        Favicon::Globe => crate::icons::GLOBE,
    };
    let _ = window.paint_svg(
        bounds,
        mark.into(),
        None,
        gpui::TransformationMatrix::default(),
        tint,
        cx,
    );
}

/// Mix `a` toward `b` by `t` in sRGB, like CSS `color-mix(in srgb, …)`.
fn mix_rgb(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    gpui::Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: lerp(a.a, b.a),
    }
    .into()
}

/// The palette's info blue (links and plugin chips).
fn info(theme: &crate::theme::Theme) -> Hsla {
    if theme.appearance.is_dark() {
        gpui::hsla(216.1 / 360.0, 0.917, 0.576, 1.0)
    } else {
        gpui::hsla(215.3 / 360.0, 0.72, 0.49, 1.0)
    }
}

/// The palette's lime (skill chips). Fixed, unlike the selectable accent.
fn lime(theme: &crate::theme::Theme) -> Hsla {
    if theme.appearance.is_dark() {
        gpui::hsla(83.0 / 360.0, 0.72, 0.46, 1.0)
    } else {
        gpui::hsla(83.0 / 360.0, 0.74, 0.43, 1.0)
    }
}

/// A link's text: info blue with a touch of the foreground.
pub(crate) fn link_color(theme: &crate::theme::Theme) -> Hsla {
    mix_rgb(theme.text, info(theme), 0.92)
}

/// A link's underline: its colour at 35%.
pub(crate) fn link_underline(theme: &crate::theme::Theme) -> Hsla {
    link_color(theme).opacity(0.35)
}

/// A skill chip's label: lime, 72% over the foreground.
pub(crate) fn skill_color(theme: &crate::theme::Theme) -> Hsla {
    mix_rgb(theme.text, lime(theme), 0.72)
}

/// A skill chip's icon: lime, 78% over the foreground.
pub(crate) fn skill_icon_color(theme: &crate::theme::Theme) -> Hsla {
    mix_rgb(theme.text, lime(theme), 0.78)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(text: &str) -> Vec<&str> {
        site_links_in_text(text)
            .into_iter()
            .map(|range| &text[range])
            .collect()
    }

    #[test]
    fn site_links_find_bare_urls_without_trailing_punctuation() {
        assert_eq!(
            found("See https://github.com/a/b/pull/12, and (http://example.com/x)."),
            ["https://github.com/a/b/pull/12", "http://example.com/x"]
        );
        assert_eq!(found("HTTPS://Example.com ok"), ["HTTPS://Example.com"]);
        // Glued to a word, a bare scheme, or quoted: not links.
        assert!(found("xhttps://example.com").is_empty());
        assert!(found("https:// nothing").is_empty());
        assert_eq!(found("<https://a.dev>"), ["https://a.dev"]);
        assert_eq!(found("ünïcode https://a.dev/é"), ["https://a.dev/é"]);
    }

    #[test]
    fn site_links_handle_unicode_near_possible_schemes() {
        for text in [
            "here’s the update",
            "hello世界",
            "h12345é",
            "h1234’é",
            "H🙂🙂",
        ] {
            assert!(found(text).is_empty(), "{text}");
        }
        assert_eq!(
            found("here’s https://example.com/é and hello世界 HTTP://example.org"),
            ["https://example.com/é", "HTTP://example.org"]
        );
        for character in ['’', 'é', '世', '🙂'] {
            for offset in 0..10 {
                let text = format!("h{}{character} https://example.com/é", "a".repeat(offset));
                assert_eq!(found(&text), ["https://example.com/é"], "{text}");
            }
        }
    }

    #[test]
    fn site_links_label_known_sites_like_the_reference() {
        let label = |url| known_site_link(url).map(|(_, label)| label);
        assert_eq!(
            label("https://github.com/opcodehq/zuse/pull/123"),
            Some("opcodehq/zuse#123".into())
        );
        assert_eq!(
            label("https://github.com/opcodehq/zuse/commit/abcdef1234"),
            Some("opcodehq/zuse@abcdef1".into())
        );
        assert_eq!(
            label("https://github.com/opcodehq/zuse"),
            Some("opcodehq/zuse".into())
        );
        assert_eq!(label("https://www.github.com/"), Some("GitHub".into()));
        assert_eq!(
            label("https://gitlab.com/group/proj/-/merge_requests/7"),
            Some("group/proj!7".into())
        );
        assert_eq!(
            label("https://linear.app/acme/issue/ENG-42/fix-login"),
            Some("ENG-42".into())
        );
        assert_eq!(
            label("https://www.figma.com/file/AbC/Design-System?node-id=1"),
            Some("Design System".into())
        );
        assert_eq!(
            label("https://www.notion.so/acme/Launch-Plan-0123456789abcdef0123456789abcdef"),
            Some("Launch Plan".into())
        );
        assert_eq!(label("https://github.com:8443/a/b"), None);
        assert_eq!(label("https://example.com/a"), None);
    }

    #[test]
    fn site_links_label_other_sites_by_host_and_path() {
        assert_eq!(
            link_label("https://www.example.com/docs/intro/?q=1#top"),
            "example.com/docs/intro"
        );
        assert_eq!(link_label("https://example.com"), "example.com");
        assert_eq!(link_label("http://localhost:3000/a"), "localhost:3000/a");
        let long = format!("https://example.com/{}", "a".repeat(80));
        assert_eq!(link_label(&long).chars().count(), MAX_LABEL_CHARS);
        assert!(link_label(&long).ends_with('…'));
    }

    #[test]
    fn site_links_pick_marks_remote_icons_or_the_globe() {
        assert_eq!(
            favicon("https://github.com/a/b"),
            Favicon::Mark(crate::icons::SITE_GITHUB)
        );
        assert_eq!(
            favicon("http://docs.rs/gpui"),
            Favicon::Remote("https://docs.rs/favicon.ico".into())
        );
        assert_eq!(favicon("http://localhost:3000"), Favicon::Globe);
        assert_eq!(favicon("http://192.168.1.4/x"), Favicon::Globe);
    }
}
