use crate::composer::ComposerInput;
use crate::settings::{self, plugins::catalog_ui, widgets};
use crate::{icons, state::AppState, theme::Theme};
use gpui::{
    AnyElement, App, Context, Entity, Global, ScrollHandle, Subscription, Task, Window, div,
    prelude::*, px,
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use zuse_protocol::WorkspaceTarget;
use zuse_protocol::skill_catalog::{
    self, CatalogAction, CatalogEntry, CatalogRequest, CatalogState, Installation, SkillFile,
};
use zuse_rpc::{RpcError, methods};

gpui::actions!(zuse_skills, [OpenSkillsPage]);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Checkout {
    device: String,
    target: WorkspaceTarget,
}

struct Destination {
    name: String,
    checkout: Checkout,
}

/// Skills worth starting with: one or two from each publisher, shown above
/// the full list while browsing without a search.
const POPULAR: &[&str] = &[
    "code-review",
    "frontend-design",
    "tdd",
    "webapp-testing",
    "agent-browser",
    "react-best-practices",
    "spec-driven-development",
    "gemini-api",
];

fn destinations(state: &AppState) -> Vec<Destination> {
    let current = state.selected_chat_row().filter(|chat| !chat.archived);
    let project = current
        .and_then(|chat| state.space_for_chat(chat))
        .or_else(|| state.selected_space_row());
    let mut spaces = state.spaces_sorted();
    spaces.retain(|space| !zuse_protocol::projectless::is_projectless_cwd(Some(&space.path)));
    spaces.sort_by_key(|space| {
        (
            project.is_none_or(|project| space.id != project.id),
            state.local_device_id.as_deref() != Some(space.device_id.as_str()),
        )
    });
    let mut choices = Vec::new();
    if let Some(chat) = current
        && let Some(space) = state.space_for_chat(chat)
        && chat.cwd.as_ref().is_some_and(|cwd| cwd != &space.path)
    {
        choices.push(Destination {
            name: format!("{} · this chat", space.display_name()),
            checkout: Checkout {
                device: chat.device_id.clone(),
                target: WorkspaceTarget {
                    chat_id: Some(chat.id.clone()),
                    space_id: None,
                    checkout_path: None,
                },
            },
        });
    }
    let mut seen = std::collections::BTreeMap::<String, usize>::new();
    for space in &spaces {
        *seen.entry(space.display_name().to_owned()).or_default() += 1;
    }
    choices.extend(spaces.into_iter().map(|space| {
        // Only the project name, unless two projects share it: then the
        // computer tells them apart.
        let name = space.display_name().to_owned();
        let name = match state.device_name(&space.device_id) {
            Some(device) if seen.get(&name).is_some_and(|&n| n > 1) => {
                format!("{name} · {device}")
            }
            _ => name,
        };
        Destination {
            name,
            checkout: Checkout {
                device: space.device_id.clone(),
                target: WorkspaceTarget {
                    chat_id: None,
                    space_id: Some(space.id.clone()),
                    checkout_path: None,
                },
            },
        }
    }));
    choices
}

pub(crate) struct SkillsPage {
    state: Entity<AppState>,
    search: Entity<ComposerInput>,
    destination: Option<Checkout>,
    select: widgets::SelectState,
    requested: Option<(Checkout, Option<usize>)>,
    snapshot: Option<CatalogState>,
    request: Option<Task<()>>,
    error: Option<String>,
    installed_only: bool,
    detail: Option<CatalogEntry>,
    preview: Option<FilePreview>,
    preview_task: Option<Task<()>>,
    stars: BTreeMap<String, u64>,
    fetched_stars: Option<Instant>,
    stars_task: Option<Task<()>>,
    browse_scroll: ScrollHandle,
    detail_scroll: ScrollHandle,
    _search: Subscription,
    _state: Subscription,
}

struct FilePreview {
    path: String,
    text: Option<Result<String, String>>,
}

struct SharedSkillsPage(Entity<SkillsPage>);
impl Global for SharedSkillsPage {}

impl SkillsPage {
    pub(crate) fn shared(state: Entity<AppState>, cx: &mut App) -> Entity<Self> {
        // One page per app keeps search and scroll across visits, but only
        // while it reads the same app state the caller owns.
        if let Some(shared) = cx.try_global::<SharedSkillsPage>()
            && shared.0.read(cx).state == state
        {
            return shared.0.clone();
        }
        let page = cx.new(|cx| {
            let search = cx.new(|cx| {
                ComposerInput::new("Search GitHub skills", cx)
                    .with_single_line()
                    .with_tab_stop()
            });
            Self {
                _search: cx.observe(&search, |_, _, cx| cx.notify()),
                _state: cx.observe(&state, |_, _, cx| cx.notify()),
                search,
                state,
                destination: None,
                select: Default::default(),
                requested: None,
                snapshot: None,
                request: None,
                error: None,
                installed_only: false,
                detail: None,
                preview: None,
                preview_task: None,
                stars: BTreeMap::new(),
                fetched_stars: None,
                stars_task: None,
                browse_scroll: ScrollHandle::new(),
                detail_scroll: ScrollHandle::new(),
            }
        });
        cx.set_global(SharedSkillsPage(page.clone()));
        page
    }

    pub(crate) fn open(&mut self, cx: &mut Context<Self>) {
        self.detail = None;
        self.preview = None;
        self.preview_task = None;
        self.installed_only = false;
        self.destination = None;
        self.requested = None;
        self.snapshot = None;
        self.request = None;
        self.error = None;
        self.browse_scroll.set_offset(gpui::point(px(0.), px(0.)));
        self.refresh_stars(false, cx);
        cx.notify();
    }

    #[cfg(feature = "browser-fixture")]
    pub(crate) fn fixture_detail(&mut self, skill: &str, cx: &mut Context<Self>) {
        self.detail = skill_catalog::entry(skill);
        cx.notify();
    }

    fn refresh_stars(&mut self, force: bool, cx: &mut Context<Self>) {
        // Tests keep the catalog's own counts: a live GitHub request finishes
        // on a Tokio worker and trips the deterministic test scheduler.
        if cfg!(test)
            || self.stars_task.is_some()
            || (!force
                && self
                    .fetched_stars
                    .is_some_and(|last| last.elapsed() < Duration::from_secs(3600)))
        {
            return;
        }
        self.fetched_stars = Some(Instant::now());
        let job = gpui_tokio::Tokio::spawn(cx, zuse_engine::github_skills::repository_stars());
        self.stars_task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |page, cx| {
                page.stars_task = None;
                if let Ok(stars) = result {
                    page.stars.extend(stars);
                }
                cx.notify();
            });
        }));
    }

    fn stars(&self, entry: CatalogEntry) -> u64 {
        self.stars
            .get(&entry.source.repository)
            .copied()
            .unwrap_or(entry.source.stars)
    }

    fn sync_destination(&mut self, destination: Option<&Checkout>, cx: &mut Context<Self>) {
        let engine = self.state.read(cx).engine();
        let key = destination.map(|target| {
            (
                target.clone(),
                engine.map(|engine| engine.client() as *const _ as usize),
            )
        });
        if self.requested == key {
            return;
        }
        self.requested = key;
        self.snapshot = None;
        self.request = None;
        self.error = None;
        if let Some(destination) = destination {
            self.request(destination.clone(), CatalogAction::List, cx);
        }
    }

    fn request(&mut self, checkout: Checkout, action: CatalogAction, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.error =
                Some("Connect to the selected checkout’s computer to manage skills.".into());
            cx.notify();
            return;
        };
        let expected_checkout_id = self
            .snapshot
            .as_ref()
            .map(|state| state.checkout_id.clone());
        let mut params = serde_json::to_value(CatalogRequest {
            target: checkout.target,
            action,
            expected_checkout_id,
        })
        .expect("serializable catalog request");
        params["targetDeviceId"] = checkout.device.into();
        self.error = None;
        self.request = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::PROJECT_SKILL_CATALOG, params)
                .await;
            let _ = this.update(cx, |page, cx| {
                page.request = None;
                match result {
                    Ok(value) => match serde_json::from_value(value) {
                        Ok(snapshot) => page.snapshot = Some(snapshot),
                        Err(_) => {
                            page.error =
                                Some("The computer returned an invalid skill catalog.".into())
                        }
                    },
                    Err(RpcError::UnknownMethod(_)) => {
                        page.error = Some(
                            "Update Zuse on the checkout’s computer to install skills.".into(),
                        );
                    }
                    Err(error) => page.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn installation(&self, entry: CatalogEntry) -> Option<Installation> {
        self.snapshot
            .as_ref()?
            .installations
            .get(&entry.package.id)
            .copied()
    }

    fn open_file(&mut self, entry: CatalogEntry, file: &'static SkillFile, cx: &mut Context<Self>) {
        self.preview_task = None;
        if self
            .preview
            .as_ref()
            .is_some_and(|preview| preview.path == file.path)
        {
            self.preview = None;
            cx.notify();
            return;
        }
        self.preview = Some(FilePreview {
            path: file.path.clone(),
            text: None,
        });
        let job = gpui_tokio::Tokio::spawn(cx, zuse_engine::github_skills::preview(entry, file));
        self.preview_task = Some(cx.spawn(async move |this, cx| {
            let result = job
                .await
                .map_err(|_| "The preview was interrupted.".to_owned())
                .and_then(|result| result)
                .map(|bytes| match String::from_utf8(bytes) {
                    Ok(text) => text,
                    Err(_) => "This file is a binary asset. View it on GitHub.".into(),
                });
            let _ = this.update(cx, |page, cx| {
                if let Some(preview) = page
                    .preview
                    .as_mut()
                    .filter(|preview| preview.path == file.path)
                {
                    preview.text = Some(result);
                }
                page.preview_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn project_picker(
        &self,
        choices: &[Destination],
        selected: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if choices.is_empty() {
            return muted("Add a project to install skills.", theme).into_any_element();
        }
        let checkouts: Vec<_> = choices
            .iter()
            .map(|choice| choice.checkout.clone())
            .collect();
        widgets::select(
            "skill-project",
            "Install in project",
            theme,
            |page: &mut Self| &mut page.select,
        )
        .compact()
        .menu_width(220.)
        .options(
            choices
                .iter()
                .map(|choice| widgets::SelectOption::new(choice.name.clone())),
            selected,
        )
        .on_select(move |page, index, _, cx| {
            page.destination = checkouts.get(index).cloned();
            cx.notify();
        })
        .render(&self.select, cx)
        .into_any_element()
    }

    fn skill_row(&self, entry: CatalogEntry, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let id = entry.package.id.as_str();
        let page = cx.entity();
        let hover = theme.glass_hover();
        let row = div()
            .id(id)
            .debug_selector(move || format!("skill-{id}"))
            .role(gpui::Role::Button)
            .aria_label(entry.package.title.clone())
            .tab_index(0)
            .focus_visible(|style| style.border_1().border_color(theme.accent))
            .cursor_pointer()
            .hover(move |row| row.bg(hover))
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(12.))
            .p(px(8.))
            .rounded(px(9.))
            .child(avatar(entry, 36., theme))
            .child(catalog_ui::title_block(
                theme,
                entry.package.title.clone(),
                entry.package.description.clone(),
                13.,
                12.,
            ))
            .child(
                div()
                    .flex_none()
                    .max_w(px(112.))
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(2.))
                    .child(star_label(self.stars(entry), theme))
                    .child(muted(entry.source.publisher.clone(), theme).truncate()),
            )
            .when(
                self.installation(entry) == Some(Installation::Installed),
                |row| {
                    row.child(
                        icons::icon(icons::CHECK)
                            .size(px(13.))
                            .flex_none()
                            .text_color(theme.success),
                    )
                },
            );
        catalog_ui::activate(row, move |_, cx| {
            page.update(cx, |page, cx| {
                page.detail = Some(entry);
                page.preview = None;
                page.preview_task = None;
                page.detail_scroll.set_offset(gpui::point(px(0.), px(0.)));
                cx.notify();
            })
        })
        .into_any_element()
    }

    fn browse(
        &self,
        picker: AnyElement,
        columns: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page = cx.entity();
        let query = self.search.read(cx).text().trim().to_lowercase();
        let mut results = skill_catalog::entries()
            .filter(|&entry| !self.installed_only || self.installation(entry).is_some())
            .filter(|entry| {
                let fields = [
                    &entry.package.title,
                    &entry.package.id,
                    &entry.package.description,
                    &entry.source.publisher,
                    &entry.source.repository,
                ]
                .map(|field| field.to_lowercase());
                query
                    .split_whitespace()
                    .all(|word| fields.iter().any(|field| field.contains(word)))
            })
            .collect::<Vec<_>>();
        results.sort_by(|&a, &b| {
            self.stars(b)
                .cmp(&self.stars(a))
                .then_with(|| a.package.title.cmp(&b.package.title))
        });
        let count = self
            .snapshot
            .as_ref()
            .map(|state| state.installations.len())
            .filter(|&n| n > 0);
        let tabs = catalog_ui::workspace_tabs(
            theme,
            "skills-filter-tab",
            "Filter skills",
            &[("Browse", None, None), ("Installed", None, count)],
            usize::from(self.installed_only),
            |index, tab| {
                let page = page.clone();
                catalog_ui::activate(tab, move |_, cx| {
                    page.update(cx, |page, cx| {
                        page.installed_only = index == 1;
                        cx.notify();
                    })
                })
            },
        );
        let body = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(16.))
                    .child(catalog_ui::page_heading(
                        theme,
                        "Skills",
                        "Agent Skills from GitHub, ranked by repository stars.",
                    ))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_wrap()
                            .gap(px(8.))
                            .items_center()
                            .child(catalog_ui::search_field(
                                theme,
                                Some(224.),
                                self.search.clone().into_any_element(),
                            ))
                            .when(self.installed_only, |toolbar| toolbar.child(picker))
                            .child(div().flex_1())
                            .child(self.refresh_button(theme, cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .flex_wrap()
                    .gap(px(8.))
                    .child(tabs)
                    .children(catalog_ui::page_tabs(theme, true)),
            )
            .children(
                self.error
                    .as_ref()
                    .filter(|_| self.installed_only)
                    .map(|error| widgets::inline_error(theme, error.clone())),
            );
        if results.is_empty() {
            let message = if self.installed_only && self.request.is_some() {
                "Checking the selected checkout…"
            } else if self.installed_only && query.is_empty() {
                "No catalog skills installed in this checkout."
            } else {
                "No skills match your search."
            };
            return body
                .child(div().p(px(9.)).child(muted(message, theme)))
                .into_any_element();
        }
        let mut section = |title: Option<&'static str>, entries: &[CatalogEntry]| {
            let rows = entries
                .chunks(columns)
                .map(|chunk| {
                    let mut row = div().flex().gap(px(16.));
                    for &entry in chunk {
                        row = row.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(self.skill_row(entry, theme, cx)),
                        );
                    }
                    if chunk.len() < columns {
                        row = row.child(div().flex_1());
                    }
                    row
                })
                .collect::<Vec<_>>();
            div()
                .id(title.unwrap_or("skill-results"))
                .flex()
                .flex_col()
                .gap(px(4.))
                .children(title.map(|title| catalog_ui::section_label(theme, title)))
                .child(div().flex().flex_col().gap(px(2.)).children(rows))
        };
        if self.installed_only || !query.is_empty() {
            return body.child(section(None, &results)).into_any_element();
        }
        let popular = POPULAR
            .iter()
            .filter_map(|id| skill_catalog::entry(id))
            .collect::<Vec<_>>();
        body.child(section(Some("Popular"), &popular))
            .child(section(Some("All skills"), &results))
            .into_any_element()
    }

    fn refresh_button(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let page = cx.entity();
        catalog_ui::activate(
            catalog_ui::icon_button(theme, "refresh-skills", icons::REFRESH, "Refresh skills"),
            move |_, cx| {
                page.update(cx, |page, cx| {
                    page.refresh_stars(true, cx);
                    if page.request.is_none()
                        && let Some((target, _)) = page.requested.clone()
                    {
                        page.request(target, CatalogAction::List, cx);
                    }
                })
            },
        )
        .into_any_element()
    }

    fn install_panel(
        &self,
        entry: CatalogEntry,
        picker: AnyElement,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let installation = self.installation(entry);
        let (label, action) = match installation {
            Some(Installation::Installed) => (
                "Remove",
                Some(CatalogAction::Remove {
                    skill: entry.package.id.clone(),
                }),
            ),
            Some(Installation::Existing) => ("Already exists", None),
            None if self.snapshot.is_some() => (
                "Install",
                Some(CatalogAction::Install {
                    skill: entry.package.id.clone(),
                }),
            ),
            None => ("Install", None),
        };
        let label = if self.request.is_some() {
            "Checking…"
        } else {
            label
        };
        let button = if label == "Install" && action.is_some() {
            crate::popover::btn_primary(theme, label)
                .id("skill-installation-action")
                .role(gpui::Role::Button)
                .tab_index(0)
        } else {
            catalog_ui::secondary_button(theme, "skill-installation-action").child(label)
        };
        let button = match (action, &self.requested) {
            (Some(action), Some((checkout, _))) if self.request.is_none() => {
                let page = cx.entity();
                let checkout = checkout.clone();
                catalog_ui::activate(button, move |_, cx| {
                    page.update(cx, |page, cx| {
                        if page.request.is_none()
                            && page
                                .requested
                                .as_ref()
                                .is_some_and(|(target, _)| target == &checkout)
                        {
                            page.request(checkout.clone(), action.clone(), cx);
                        }
                    })
                })
            }
            _ => button.opacity(0.6).cursor_default().tab_index(-1),
        };
        let (title, note) = match installation {
            Some(Installation::Installed) => (
                "Installed in",
                "Type /skills in a chat in this project to use it.".to_owned(),
            ),
            Some(Installation::Existing) => (
                "Already in",
                format!("{} already has other files", entry.install_path()),
            ),
            None => ("Install in", "Adds it to .agents/skills".to_owned()),
        };
        let row = crate::frost::layered(
            div()
                .relative()
                .w_full()
                .min_w_0()
                .px(px(14.))
                .py(px(11.))
                .rounded(px(9.))
                .bg(theme.surface_card)
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(120.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text_strong)
                                .child(title),
                        )
                        .child(muted(note, theme).truncate()),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(picker)
                        .child(button),
                ),
        );
        div()
            .relative()
            .w_full()
            .min_w_0()
            .rounded(px(12.))
            .border_1()
            .border_color(theme.border)
            .child(crate::shell::announcement_background(
                theme,
                theme.accent,
                12.,
                12.,
            ))
            .child(
                div()
                    .relative()
                    .w_full()
                    .min_w_0()
                    .px(px(20.))
                    .py(px(22.))
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.text_strong)
                            .child(format!("Set up {}", entry.package.title)),
                    )
                    .child(row),
            )
            .into_any_element()
    }

    fn file_list(&self, entry: CatalogEntry, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut files = entry.package.files.iter().collect::<Vec<_>>();
        files.sort_by_key(|file| (file.path != "SKILL.md", file.path.as_str()));
        let page = cx.entity();
        let tile = catalog_ui::control_fill(theme);
        let rows = files
            .into_iter()
            .map(|file| {
                let expanded = self
                    .preview
                    .as_ref()
                    .filter(|preview| preview.path == file.path);
                let entity = page.clone();
                let hover = theme.glass_hover();
                let row = div()
                    .id(file.path.as_str())
                    .w_full()
                    .min_w_0()
                    .px(px(16.))
                    .py(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .cursor_pointer()
                    .role(gpui::Role::Button)
                    .tab_index(0)
                    .aria_expanded(expanded.is_some())
                    .hover(move |style| style.bg(hover))
                    .child(
                        div()
                            .size(px(28.))
                            .flex_none()
                            .rounded(px(7.))
                            .bg(tile)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                icons::icon(icons::FILE_CODE)
                                    .size(px(14.))
                                    .text_color(theme.text_muted),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.))
                                    .text_color(theme.text_strong)
                                    .child(file.path.clone()),
                            )
                            .child(muted(size_label(file.bytes), theme)),
                    )
                    .child(
                        icons::icon(if expanded.is_some() {
                            icons::ALT_ARROW_DOWN
                        } else {
                            icons::ALT_ARROW_RIGHT
                        })
                        .size(px(13.))
                        .flex_none()
                        .text_color(theme.text_muted),
                    );
                div()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(catalog_ui::activate(row, move |_, cx| {
                        entity.update(cx, |page, cx| page.open_file(entry, file, cx))
                    }))
                    .children(expanded.map(|preview| {
                        let contents = match &preview.text {
                            Some(Ok(text)) => text.chars().take(28_000).collect::<String>(),
                            Some(Err(error)) => error.clone(),
                            None => "Loading from GitHub…".into(),
                        };
                        div()
                            .w_full()
                            .min_w_0()
                            .px(px(16.))
                            .py(px(12.))
                            .bg(theme.ink(0.035))
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .font_family(theme.font_mono.clone())
                            .children(
                                contents
                                    .lines()
                                    .map(|line| div().min_h(px(18.)).child(line.to_owned())),
                            )
                    }))
            })
            .collect::<Vec<_>>();
        let count = entry.package.files.len();
        panel(theme)
            .overflow_hidden()
            .child(
                div()
                    .px(px(16.))
                    .py(px(12.))
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(format!(
                        "{count} {}",
                        if count == 1 { "file" } else { "files" }
                    )),
            )
            .children(rows)
            .into_any_element()
    }

    fn back_button(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let page = cx.entity();
        let hover = theme.glass_hover();
        let strong = theme.text_strong;
        let button = div()
            .id("skill-back")
            .self_start()
            .h(px(26.))
            .ml(px(-8.))
            .pl(px(6.))
            .pr(px(10.))
            .rounded(px(7.))
            .flex()
            .items_center()
            .gap(px(5.))
            .text_size(px(12.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.text_muted)
            .cursor_pointer()
            .role(gpui::Role::Button)
            .aria_label("Back to all skills")
            .tab_index(0)
            .focus_visible(|style| style.border_1().border_color(theme.accent))
            .hover(move |style| style.bg(hover).text_color(strong))
            .child(
                icons::icon(icons::ALT_ARROW_LEFT)
                    .size(px(14.))
                    .text_color(theme.text_muted),
            )
            .child("Skills");
        catalog_ui::activate(button, move |_, cx| {
            page.update(cx, |page, cx| {
                page.detail = None;
                page.preview = None;
                page.preview_task = None;
                cx.notify();
            })
        })
        .into_any_element()
    }

    fn detail(
        &self,
        entry: CatalogEntry,
        picker: AnyElement,
        wide: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let url = entry.repository_url();
        let source_url = format!(
            "{url}/tree/{}/{}",
            entry.source.revision, entry.package.path
        );
        let fact = |label: &'static str, value: AnyElement| {
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(muted(label, theme))
                .child(value)
        };
        let about = div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(16.))
            .text_size(px(13.))
            .child(fact(
                "GitHub stars",
                star_label(self.stars(entry), theme)
                    .text_size(px(13.))
                    .text_color(theme.text)
                    .into_any_element(),
            ))
            .child(fact(
                "Repository",
                github_link(theme, "skill-repository", &entry.source.repository, url),
            ))
            .child(fact(
                "Made by",
                div()
                    .text_color(theme.text)
                    .child(entry.source.publisher.clone())
                    .into_any_element(),
            ))
            .child(fact(
                "Source",
                github_link(
                    theme,
                    "skill-revision",
                    &entry.source.revision[..7],
                    source_url,
                ),
            ))
            .child(div().h(px(1.)).w_full().bg(theme.border))
            .child(fact(
                "Installs to",
                div()
                    .min_w_0()
                    .text_size(px(12.))
                    .font_family(theme.font_mono.clone())
                    .text_color(theme.text)
                    .child(entry.install_path())
                    .into_any_element(),
            ));
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(self.back_button(theme, cx))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(avatar(entry, 48., theme))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .debug_selector(|| "skill-detail-heading".into())
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(20.))
                                    .line_height(crate::typography::ui_rems(28.))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.text_strong)
                                    .child(entry.package.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .line_height(px(19.))
                                    .text_color(theme.text_muted)
                                    .child(entry.package.description.clone()),
                            ),
                    ),
            )
            .children(
                self.error
                    .as_ref()
                    .map(|error| widgets::inline_error(theme, error.clone())),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .gap(px(32.))
                    .items_start()
                    .when(!wide, |row| row.flex_col().gap(px(24.)))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .when(!wide, |col| col.w_full())
                            .flex()
                            .flex_col()
                            .gap(px(16.))
                            .child(self.install_panel(entry, picker, theme, cx))
                            .child(self.file_list(entry, theme, cx)),
                    )
                    .child(
                        about
                            .when(wide, |col| col.w(px(180.)).flex_none().pt(px(4.)))
                            .when(!wide, |col| col.w_full()),
                    ),
            )
            .into_any_element()
    }
}

impl Render for SkillsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        self.refresh_stars(false, cx);
        let choices = destinations(self.state.read(cx));
        let selected = choices
            .iter()
            .position(|choice| Some(&choice.checkout) == self.destination.as_ref())
            .unwrap_or(0);
        self.sync_destination(choices.get(selected).map(|choice| &choice.checkout), cx);
        let picker = self.project_picker(&choices, selected, &theme, cx);
        let prefs = settings::current(cx);
        let sidebar = if prefs.sidebar_collapsed {
            0.
        } else {
            prefs.sidebar_width
        };
        let width = f32::from(window.viewport_size().width) - sidebar;
        let padding = catalog_ui::page_padding(width);
        let (scroll, content) = if let Some(entry) = self.detail {
            (
                &self.detail_scroll,
                self.detail(entry, picker, width >= 810., &theme, cx),
            )
        } else {
            let columns = if width < 730. { 1 } else { 2 };
            (
                &self.browse_scroll,
                self.browse(picker, columns, &theme, cx),
            )
        };
        div()
            .id("skills-page")
            .debug_selector(|| "skills-page".into())
            .size_full()
            .overflow_y_scroll()
            .track_scroll(scroll)
            .text_color(theme.text)
            .text_size(crate::typography::text_sm())
            .child(
                div()
                    .w_full()
                    .max_w(px(catalog_ui::PAGE_MAX_WIDTH))
                    .min_w_0()
                    .mx_auto()
                    .px(px(padding))
                    .py(px(32.))
                    .child(content),
            )
    }
}

fn muted(value: impl Into<gpui::SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .text_size(px(12.))
        .text_color(theme.text_muted)
        .child(value.into())
}

fn panel(theme: &Theme) -> gpui::Div {
    div()
        .w_full()
        .min_w_0()
        .rounded(px(11.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface_card)
        .flex()
        .flex_col()
}

fn avatar(entry: CatalogEntry, size: f32, theme: &Theme) -> gpui::Div {
    let background = catalog_ui::control_fill(theme);
    let initial = entry
        .source
        .publisher
        .chars()
        .next()
        .unwrap_or('?')
        .to_string();
    let fallback = move || {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(background)
            .child(initial.clone())
            .into_any_element()
    };
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(9.))
        .overflow_hidden()
        .bg(background)
        .child(
            gpui::img(gpui::SharedString::from(entry.avatar_url()))
                .size_full()
                .rounded(px(9.))
                .with_loading(fallback.clone())
                .with_fallback(fallback),
        )
}

fn star_label(stars: u64, theme: &Theme) -> gpui::Div {
    let value = if stars >= 1000 {
        format!("{:.1}k", stars as f64 / 1000.)
    } else {
        stars.to_string()
    };
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .text_size(px(12.))
        .text_color(theme.text_muted)
        .child(
            icons::icon(icons::STAR)
                .size(px(12.))
                .text_color(theme.text_muted),
        )
        .child(value)
}

fn size_label(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.)
    }
}

fn github_link(theme: &Theme, id: &'static str, label: &str, url: String) -> AnyElement {
    let mut link = div()
        .id(id)
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(4.))
        .cursor_pointer()
        .role(gpui::Role::Link)
        .tab_index(0)
        .text_color(theme.text);
    link = link
        .child(div().min_w_0().truncate().child(label.to_owned()))
        .child(
            icons::icon(icons::ARROW_UP_RIGHT)
                .size(px(12.))
                .flex_none()
                .text_color(theme.text_muted),
        );
    catalog_ui::activate(link, move |_, cx| cx.open_url(&url)).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    struct CatalogWindow(Entity<SkillsPage>);
    impl Render for CatalogWindow {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0.clone()
        }
    }

    #[test]
    fn popular_skills_are_all_in_the_catalog() {
        for id in POPULAR {
            assert!(skill_catalog::entry(id).is_some(), "{id}");
        }
    }

    #[gpui::test]
    fn skill_detail_title_has_room_at_desktop_and_narrow_widths(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            settings::init(settings::UiSettings::default(), directory.path(), cx);
        });
        let (_, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| AppState::new());
            let page = SkillsPage::shared(state, cx);
            page.update(cx, |page, _| {
                page.fetched_stars = Some(Instant::now());
                page.detail = skill_catalog::entry("cloud-logging-query-generation");
            });
            CatalogWindow(page)
        });
        for width in [420., 1180.] {
            cx.simulate_resize(gpui::size(px(width), px(700.)));
            cx.update(|window, cx| window.draw(cx).clear());
            let heading = cx.debug_bounds("skill-detail-heading").unwrap();
            assert!(heading.size.width > px(160.), "{heading:?}");
            assert!(heading.size.height < px(135.), "{heading:?}");
        }
    }
}
