//! Repository artwork is fetched once per project/connection, off the render thread.
use super::*;
use crate::files::client::{FilesRequestContext, WorkspaceFilesClient};
use crate::image_media::{MediaImage, decode_project_icon, release_media};

pub(super) const ICON_PATHS: &[&str] = &[
    "public/apple-touch-icon.png",
    "apple-touch-icon.png",
    "public/favicon.svg",
    "favicon.svg",
    "public/favicon.png",
    "public/icon.png",
    "public/logo.png",
    "favicon.png",
    "app/icon.png",
    "src/app/icon.png",
    "public/favicon.ico",
    "favicon.ico",
    "app/favicon.ico",
    "static/favicon.ico",
    "src-tauri/icons/icon.png",
    "assets/icon.png",
    "src/assets/icon.png",
];

fn load_local_icon(root: &std::path::Path) -> Option<MediaImage> {
    // A project can point at a subdirectory; match the remote workspace RPC's
    // checkout-root resolution, including linked worktrees with a .git file.
    let canonical = root.canonicalize().ok()?;
    let root = canonical
        .ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(&canonical);
    for path in ICON_PATHS {
        let path = root.join(path);
        if !path.is_file() {
            continue;
        }
        let bytes = std::fs::File::open(&path).ok().and_then(|file| {
            use std::io::Read;
            let mut bytes = Vec::new();
            file.take(zuse_protocol::MAX_WORKSPACE_IMAGE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .ok()?;
            Some(bytes)
        })?;
        let mime = if path.extension().is_some_and(|e| e == "svg") {
            "image/svg+xml"
        } else {
            "image/png"
        };
        return decode_project_icon(mime, bytes).ok();
    }
    None
}

// Curated badge tones: (dark appearance, light appearance). Keep the ordering
// stable so projects retain their assigned color. These are explicit colors,
// independent of the selected theme accent; only their appearance variant changes.
const MONOGRAM_PALETTE: [(u32, u32); 8] = [
    (0x94a3b8, 0x475569), // slate
    (0x93c5fd, 0x2563eb), // blue
    (0xc4b5fd, 0x7c3aed), // violet
    (0xfda4af, 0xbe123c), // rose
    (0xfcd34d, 0xa16207), // amber
    (0x6ee7b7, 0x047857), // emerald
    (0x5eead4, 0x0f766e), // teal
    (0xfdba74, 0xc2410c), // orange
];

/// A stable tone from the monogram palette for `seed`, in the current
/// appearance (the workspace switcher's square avatar).
pub(super) fn monogram_tone(seed: &str, theme: &Theme) -> gpui::Hsla {
    let hash = seed.bytes().fold(2166136261u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16777619)
    });
    let (dark, light) = MONOGRAM_PALETTE[hash as usize % MONOGRAM_PALETTE.len()];
    gpui::rgb(if theme.appearance.is_dark() {
        dark
    } else {
        light
    })
    .into()
}

fn monogram(name: &str, seed: &str, selected: bool, theme: &Theme) -> AnyElement {
    // FNV-1a selects a stable entry from the curated palette.
    let hash = seed.bytes().fold(2166136261u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16777619)
    });
    let letter = name
        .trim()
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let (dark, light) = MONOGRAM_PALETTE[hash as usize % MONOGRAM_PALETTE.len()];
    let tone = gpui::Hsla::from(gpui::rgb(if theme.appearance.is_dark() {
        dark
    } else {
        light
    }));
    let mut hover_text = tone;
    hover_text.l = if theme.appearance.is_dark() {
        (tone.l + 0.12).min(0.95)
    } else {
        (tone.l - 0.10).max(0.15)
    };
    let tile = div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        // The active row wears the hover tint permanently.
        .bg(tone.opacity(if selected { 0.24 } else { 0.08 }))
        .text_color(if selected {
            hover_text
        } else {
            tone.opacity(0.85)
        })
        .group_hover("sidebar-session-row", move |style| {
            style.bg(tone.opacity(0.24)).text_color(hover_text)
        })
        .font_family(theme.font_mono.clone())
        .font_weight(gpui::FontWeight::MEDIUM)
        .child(
            div()
                .w_full()
                .text_center()
                .text_size(px(9.0))
                .line_height(px(13.0))
                .child(letter),
        );
    crate::frost::frosted(3.0, crate::frost::MENU_BLUR, tile).into_any_element()
}

pub(super) struct ProjectIcon {
    name: String,
    seed: String,
    media: Option<MediaImage>,
    refreshed: std::time::Instant,
    _task: Task<()>,
}

impl ProjectIcon {
    fn new(
        name: String,
        seed: String,
        context: FilesRequestContext,
        engine: Option<crate::state::EngineHandle>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|this, cx| release_media(this.media.take(), cx))
            .detach();
        let task = cx.spawn(async move |this, cx| {
            let executor = cx.background_executor().clone();
            let load = async {
                if context.target_device_id.is_none() {
                    let root = std::path::PathBuf::from(context.cwd);
                    return executor.spawn(async move { load_local_icon(&root) }).await;
                }
                let client = WorkspaceFilesClient::new(engine?, context.clone());
                for path in ICON_PATHS {
                    // This also resolves the current checkout identity on the owning host.
                    let file = match client
                        .read_file(zuse_protocol::ReadWorkspaceFileRequest {
                            target: context.target.clone(),
                            path: (*path).into(),
                        })
                        .await
                    {
                        Ok(file) => file,
                        Err(error) if error.retryable() => return None,
                        Err(_) => continue,
                    };
                    let (mime, bytes) = client
                        .read_image((*path).into(), file.checkout_id)
                        .await
                        .ok()?;
                    return executor
                        .spawn(async move { decode_project_icon(&mime, bytes).ok() })
                        .await;
                }
                None
            };
            let media = match futures::future::select(
                Box::pin(load),
                Box::pin(executor.timer(Duration::from_secs(30))),
            )
            .await
            {
                futures::future::Either::Left((media, _)) => {
                    media.map(|media| media.for_view((16.0, 16.0), 2.0, 4096))
                }
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                this.media = media;
                cx.notify();
            });
        });
        Self {
            name,
            seed,
            media: None,
            refreshed: std::time::Instant::now(),
            _task: task,
        }
    }
}

impl Render for ProjectIcon {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.media {
            Some(media) => gpui::img(media.image.clone())
                .size_full()
                .object_fit(gpui::ObjectFit::Contain)
                .into_any_element(),
            None => monogram(&self.name, &self.seed, false, Theme::of(cx)),
        }
    }
}

/// Same card as the pull-request badge tooltip: project name, then the
/// owning device below in the muted line.
struct ProjectIconTooltip {
    name: SharedString,
    device: SharedString,
}

impl Render for ProjectIconTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let card = div()
            .max_w(px(320.0))
            .px(px(9.0))
            .py(px(7.0))
            .flex()
            .flex_col()
            .gap(px(3.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border_strong)
            .bg(crate::popover::surface_bg(theme))
            .when(!theme.is_frost(), |el| el.shadow_md())
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .whitespace_nowrap()
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(self.name.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .whitespace_nowrap()
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .child(self.device.clone()),
            );
        crate::frost::frosted(6.0, crate::frost::MENU_BLUR, card)
    }
}

fn project_icon_frame(
    chat_id: &str,
    name: &str,
    device: &str,
    size: f32,
    child: impl IntoElement,
) -> AnyElement {
    let name: SharedString = name.to_owned().into();
    let device: SharedString = device.to_owned().into();
    div()
        .id(SharedString::from(format!("project-icon-{chat_id}")))
        .size(px(size))
        .flex_none()
        .tooltip(move |_, cx| {
            cx.new(|_| ProjectIconTooltip {
                name: name.clone(),
                device: device.clone(),
            })
            .into()
        })
        .tooltip_show_delay(Duration::from_millis(350))
        .child(child)
        .into_any_element()
}

impl Shell {
    pub(super) fn render_project_icon(
        &self,
        chat_id: &str,
        size: f32,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        let chat = state.chats.iter().find(|chat| chat.id == chat_id);
        // The tooltip names this row's own project; the artwork is shared.
        let name = chat
            .and_then(|chat| state.space_for_chat(chat))
            .map(|space| space.display_name().to_string())
            .unwrap_or_else(|| "No project".into());
        // Same fallback as the row's "@ device" fragment.
        let device = chat
            .and_then(|chat| state.device_name(&chat.device_id))
            .unwrap_or("Unknown device")
            .to_string();
        let art = self.project_icon_art(chat_id, selected, cx);
        project_icon_frame(chat_id, &name, &device, size, art)
    }

    /// Artwork for a chat's project, shared by every space of the project:
    /// the representative's name and color, read from this device's checkout
    /// when there is one (no round trip), else from the representative.
    fn project_icon_art(
        &self,
        chat_id: &str,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        let space = state
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .and_then(|chat| state.space_for_chat(chat))
            .cloned();
        self.project_artwork(space.as_ref(), selected, cx)
    }

    /// A project's artwork: its GitHub owner's avatar when the repository is
    /// hosted on github.com, over the repository's own icon (or monogram),
    /// which shows while the avatar loads and whenever it can't be fetched.
    pub(super) fn project_artwork(
        &self,
        space: Option<&zuse_protocol::Space>,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let avatar = space.and_then(|space| github_avatar_url(self.state.read(cx), space));
        let fallback = self.project_icons.render(
            &self.state,
            space,
            self.active_sidebar_pin_profile_key(cx),
            selected,
            cx,
        );
        match avatar {
            None => fallback,
            Some(url) => div()
                .relative()
                .size_full()
                .child(fallback)
                .child(
                    gpui::img(SharedString::from(url))
                        .absolute()
                        .inset_0()
                        .size_full()
                        .rounded(px(4.0))
                        .object_fit(gpui::ObjectFit::Cover),
                )
                .into_any_element(),
        }
    }
}

/// `owner` of a github.com repository path (`owner/repo…`), when it is a
/// plausible account name.
fn github_owner(path: &str) -> Option<&str> {
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let repository = parts.next()?;
    let valid = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (valid(owner) && !repository.is_empty() && !owner.starts_with('.')).then_some(owner)
}

/// The GitHub owner avatar for a project hosted on github.com. The owner
/// comes from a remote-shaped repository identity (`github.com/owner/repo`),
/// else from a pull request one of the project's sessions reported.
pub(super) fn github_avatar_url(state: &AppState, space: &zuse_protocol::Space) -> Option<String> {
    let members = state.project_members(space);
    let owner = members
        .iter()
        .find_map(|member| {
            member
                .repository_id
                .as_deref()
                .and_then(|id| id.strip_prefix("github.com/"))
                .and_then(github_owner)
                .map(str::to_owned)
        })
        .or_else(|| {
            let ids: std::collections::HashSet<&str> =
                members.iter().map(|member| member.id.as_str()).collect();
            state
                .chats
                .iter()
                .filter(|chat| chat.space_id.as_deref().is_some_and(|id| ids.contains(id)))
                .find_map(|chat| {
                    let summary = state.change_request_for_chat(chat)?;
                    summary
                        .url
                        .strip_prefix("https://github.com/")
                        .and_then(github_owner)
                        .map(str::to_owned)
                })
        })?;
    Some(format!("https://github.com/{owner}.png?size=80"))
}

/// Shared artwork pipeline for sidebar and repository picker. Each owner keeps
/// the entities alive; loading stays off the render thread and has a deadline.
#[derive(Default)]
pub(crate) struct ProjectIcons {
    entries: std::cell::RefCell<std::collections::HashMap<String, Entity<ProjectIcon>>>,
}

impl ProjectIcons {
    pub(crate) fn render<V: 'static>(
        &self,
        state: &Entity<AppState>,
        space: Option<&zuse_protocol::Space>,
        profile: Option<String>,
        selected: bool,
        cx: &mut Context<V>,
    ) -> AnyElement {
        let state = state.read(cx);
        let representative = space.map(|space| state.representative_space(space));
        let name = representative
            .map(|space| space.display_name().to_string())
            .unwrap_or_else(|| "Home".into());
        let seed = representative
            .map(|space| space.path.clone())
            .unwrap_or_else(|| "home".into());
        let artwork = space.map(|space| {
            state
                .project_members(space)
                .into_iter()
                .find(|member| state.local_device_id.as_deref() == Some(&member.device_id))
                .unwrap_or_else(|| state.representative_space(space))
        });
        let context = artwork.map(|space| FilesRequestContext {
            target: zuse_protocol::WorkspaceTarget {
                chat_id: None,
                space_id: Some(space.id.clone()),
                checkout_path: None,
            },
            target_device_id: (state.local_device_id.as_deref() != Some(&space.device_id))
                .then(|| space.device_id.clone()),
            cwd: space.path.clone(),
            checkout_id: space.checkout_id.clone(),
        });
        let Some(context) = context else {
            return monogram(&name, &seed, selected, Theme::of(cx));
        };
        let key = format!(
            "{:?}:{:?}:{}:{:?}:{}",
            profile, context.target_device_id, context.cwd, context.checkout_id, name
        );
        let engine = state.engine().cloned();
        // Don't cache a remote miss before a connection exists.
        if context.target_device_id.is_some() && engine.is_none() {
            return monogram(&name, &seed, selected, Theme::of(cx));
        }
        let mut cache = self.entries.borrow_mut();
        cache.retain(|_, entity| entity.read(cx).refreshed.elapsed() < Duration::from_secs(300));
        let entity = cache
            .entry(key)
            .or_insert_with(|| {
                let entity =
                    cx.new(|cx| ProjectIcon::new(name.clone(), seed.clone(), context, engine, cx));
                // The owner draws the selected monogram and redraws when artwork lands.
                cx.observe(&entity, |_, _, cx| cx.notify()).detach();
                entity
            })
            .clone();
        drop(cache);
        if entity.read(cx).media.is_none() {
            return monogram(&name, &seed, selected, Theme::of(cx));
        }
        entity.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn png(path: &std::path::Path, width: u32) {
        image::RgbaImage::new(width, 2).save(path).unwrap();
    }
    #[test]
    fn sidebar_project_icon_priority_and_missing_fallback() {
        let temp = tempfile::tempdir().unwrap();
        assert!(load_local_icon(temp.path()).is_none());
        png(&temp.path().join("favicon.png"), 3);
        assert_eq!(load_local_icon(temp.path()).unwrap().width, 3.0);
        std::fs::create_dir(temp.path().join("public")).unwrap();
        png(&temp.path().join("public/apple-touch-icon.png"), 7);
        assert_eq!(load_local_icon(temp.path()).unwrap().width, 7.0);
        std::fs::write(temp.path().join(".git"), "gitdir: /unused-test-checkout").unwrap();
        std::fs::create_dir_all(temp.path().join("src/nested")).unwrap();
        assert_eq!(
            load_local_icon(&temp.path().join("src/nested"))
                .unwrap()
                .width,
            7.0
        );
        // A corrupt first match uses the default rather than showing unrelated lower-priority art.
        std::fs::write(temp.path().join("public/apple-touch-icon.png"), b"invalid").unwrap();
        assert!(load_local_icon(temp.path()).is_none());
    }
    #[test]
    fn github_owner_accepts_account_paths_only() {
        assert_eq!(github_owner("example/fieldnotes"), Some("example"));
        assert_eq!(github_owner("opcodehq/zuse/pull/412"), Some("opcodehq"));
        assert_eq!(github_owner("example"), None);
        assert_eq!(github_owner("../etc/passwd"), None);
        assert_eq!(github_owner("a b/repo"), None);
    }
    #[test]
    fn sidebar_project_icons_support_svg_and_ico() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("favicon.svg"), br##"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="8"><rect width="12" height="8" fill="#f00"/></svg>"##).unwrap();
        assert_eq!(load_local_icon(temp.path()).unwrap().width, 12.0);
        std::fs::remove_file(temp.path().join("favicon.svg")).unwrap();
        image::RgbaImage::new(16, 16)
            .save(temp.path().join("favicon.ico"))
            .unwrap();
        assert_eq!(load_local_icon(temp.path()).unwrap().width, 16.0);
    }
}
