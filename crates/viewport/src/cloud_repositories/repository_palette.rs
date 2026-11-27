//! Adding repositories: a centred command palette picker. A search field,
//! repositories grouped by GitHub account with avatars and how recently they
//! changed, multi-select with Space, Enter to add, and the GitHub actions
//! (manage access, install on another account, refresh) at the bottom.
use super::presentation::{ago, avatar, installation_url};
use super::*;
use crate::cloud_settings::kit;
use zuse_protocol::cloud_projects::GithubInstallation;

/// One navigable palette entry, kept by identity so results arriving around
/// it never move the highlight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Item {
    /// A repository, by `owner/name`.
    Repo(String),
    /// Manage repository access on GitHub, per installation.
    Manage(i64),
    Install,
    Refresh,
}

/// A repository row.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Choice {
    pub name: String,
    pub avatar: Option<String>,
    pub updated_at: String,
    /// Already a cloud repository (or being added): checked and disabled.
    pub connected: bool,
    /// Byte ranges of `name` the search matched.
    pub matched: Vec<std::ops::Range<usize>>,
    score: i64,
}

/// One GitHub account's repositories.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Group {
    pub owner: String,
    /// Repositories that arrived since the palette last saw this account.
    pub fresh: usize,
    pub choices: Vec<Choice>,
}

/// Case-insensitive fuzzy match of `query` against `text`: every query
/// character in order. Whitespace in the query is ignored. Substrings beat
/// scattered letters, the repository part beats the owner, and letters at
/// word starts score higher. Returns the score and matched byte ranges.
pub(super) fn fuzzy(query: &str, text: &str) -> Option<(i64, Vec<std::ops::Range<usize>>)> {
    let query: String = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if query.is_empty() {
        return Some((0, vec![]));
    }
    let lower = text.to_lowercase();
    // Lowercasing must keep byte offsets for highlights; non-ASCII names
    // that change length fall back to scattered matching on the original.
    let same_len = lower.len() == text.len();
    let slash = text.find('/').map_or(0, |i| i + 1);
    if same_len {
        let found = lower[slash..]
            .find(&query)
            .map(|i| (i + slash, 600))
            .or_else(|| lower.find(&query).map(|i| (i, 400)));
        if let Some((start, bonus)) = found {
            let boundary =
                start == 0 || start == slash || is_separator(lower.as_bytes()[start - 1]);
            let score = bonus + if boundary { 100 } else { 0 } - start as i64
                + if start == slash && query.len() == text.len() - slash {
                    200
                } else {
                    0
                };
            return Some((score, vec![start..start + query.len()]));
        }
    }
    let mut ranges: Vec<std::ops::Range<usize>> = vec![];
    let mut wanted = query.chars().peekable();
    let mut score = 0i64;
    let mut prev: Option<char> = None;
    let mut last_end: Option<usize> = None;
    for (i, ch) in text.char_indices() {
        let Some(&want) = wanted.peek() else {
            break;
        };
        if ch.to_lowercase().eq(want.to_lowercase()) {
            wanted.next();
            let end = i + ch.len_utf8();
            if prev.is_none_or(|p| is_separator(p as u8)) {
                score += 15;
            }
            match ranges.last_mut() {
                Some(range) if last_end == Some(i) => {
                    range.end = end;
                    score += 10;
                }
                _ => ranges.push(i..end),
            }
            last_end = Some(end);
            score += 1;
        }
        prev = Some(ch);
    }
    if wanted.peek().is_some() {
        return None;
    }
    Some((score - ranges.len() as i64 * 5, ranges))
}

fn is_separator(byte: u8) -> bool {
    matches!(byte, b'/' | b'-' | b'_' | b'.' | b' ')
}

fn owner_of(name: &str) -> &str {
    name.split('/').next().unwrap_or(name)
}

/// The palette's repository groups. Accounts keep their installation order
/// (others after, by name); within one, repositories you can still add come
/// first, newest change first, and connected ones follow. A search keeps the
/// accounts' order and ranks each account's matches best first.
pub(super) fn groups(
    catalog: &GithubCatalog,
    connected: &BTreeSet<String>,
    fresh: &BTreeSet<String>,
    query: &str,
) -> Vec<Group> {
    let searching = !query.trim().is_empty();
    let rank = |owner: &str| {
        catalog
            .installations
            .iter()
            .position(|i| i.account_login.eq_ignore_ascii_case(owner))
            .unwrap_or(usize::MAX)
    };
    let mut groups: Vec<Group> = vec![];
    for repo in &catalog.repositories {
        let Some((score, matched)) = fuzzy(query, &repo.name_with_owner) else {
            continue;
        };
        let lower = repo.name_with_owner.to_lowercase();
        let owner = owner_of(&repo.name_with_owner);
        let choice = Choice {
            name: repo.name_with_owner.clone(),
            avatar: repo.owner_avatar_url.clone().or_else(|| {
                catalog
                    .installations
                    .iter()
                    .find(|i| i.account_login.eq_ignore_ascii_case(owner))
                    .and_then(|i| i.avatar_url.clone())
            }),
            updated_at: repo.updated_at.clone(),
            connected: connected.contains(&lower),
            matched,
            score,
        };
        let is_fresh = fresh.contains(&lower) && !choice.connected;
        match groups
            .iter_mut()
            .find(|g| g.owner.eq_ignore_ascii_case(owner))
        {
            Some(group) => {
                group.fresh += usize::from(is_fresh);
                group.choices.push(choice);
            }
            None => groups.push(Group {
                owner: owner.to_owned(),
                fresh: usize::from(is_fresh),
                choices: vec![choice],
            }),
        }
    }
    groups.sort_by(|a, b| {
        rank(&a.owner)
            .cmp(&rank(&b.owner))
            .then_with(|| a.owner.to_lowercase().cmp(&b.owner.to_lowercase()))
    });
    for group in &mut groups {
        group.choices.sort_by(|a, b| {
            a.connected
                .cmp(&b.connected)
                .then_with(|| {
                    if searching {
                        b.score.cmp(&a.score)
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.name.cmp(&b.name))
        });
    }
    groups
}

/// The GitHub actions under the list: managing each installation's access,
/// installing on another account, refreshing.
pub(super) fn actions(installations: &[GithubInstallation], can_install: bool) -> Vec<Item> {
    let mut items: Vec<Item> = installations
        .iter()
        .filter(|i| !i.suspended)
        .map(|i| Item::Manage(i.installation_id))
        .collect();
    if can_install {
        items.push(Item::Install);
    }
    items.push(Item::Refresh);
    items
}

/// Everything the keyboard can land on, top to bottom. Connected
/// repositories are shown but skipped.
pub(super) fn navigable(groups: &[Group], actions: &[Item]) -> Vec<Item> {
    groups
        .iter()
        .flat_map(|g| g.choices.iter())
        .filter(|c| !c.connected)
        .map(|c| Item::Repo(c.name.clone()))
        .chain(actions.iter().cloned())
        .collect()
}

/// The palette's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Key {
    Up,
    Down,
    Toggle,
    Submit,
    Close,
}

/// What a key does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Move(Item),
    Toggle(String),
    /// Add these repositories, in list order.
    Add(Vec<String>),
    Run(Item),
    Close,
    /// The key belongs to the search field (Space with nothing to toggle).
    Pass,
    Nothing,
}

/// Up/Down wrap through `items`; Space toggles the highlighted repository;
/// Enter runs a highlighted action, else adds the selection, else the
/// highlighted repository; Esc closes.
pub(super) fn on_key(
    key: Key,
    items: &[Item],
    active: Option<&Item>,
    selected: &BTreeSet<String>,
) -> Outcome {
    let at = active.and_then(|a| items.iter().position(|i| i == a));
    match key {
        Key::Up | Key::Down if items.is_empty() => Outcome::Nothing,
        Key::Up => Outcome::Move(match at {
            Some(0) | None => items[items.len() - 1].clone(),
            Some(i) => items[i - 1].clone(),
        }),
        Key::Down => Outcome::Move(match at {
            Some(i) if i + 1 < items.len() => items[i + 1].clone(),
            _ => items[0].clone(),
        }),
        Key::Toggle => match at.map(|i| &items[i]) {
            Some(Item::Repo(name)) => Outcome::Toggle(name.clone()),
            _ => Outcome::Pass,
        },
        Key::Submit => match at.map(|i| &items[i]) {
            Some(item @ (Item::Manage(_) | Item::Install | Item::Refresh)) => {
                Outcome::Run(item.clone())
            }
            _ if !selected.is_empty() => Outcome::Add(
                items
                    .iter()
                    .filter_map(|i| match i {
                        Item::Repo(name) if selected.contains(name) => Some(name.clone()),
                        _ => None,
                    })
                    .collect(),
            ),
            Some(Item::Repo(name)) => Outcome::Add(vec![name.clone()]),
            _ => Outcome::Nothing,
        },
        Key::Close => Outcome::Close,
    }
}

/// "18 repositories from opcodehq are now available", from new
/// repositories counted per account.
pub(super) fn arrivals_message(arrivals: &[(String, usize)]) -> Option<String> {
    let total: usize = arrivals.iter().map(|(_, n)| n).sum();
    match arrivals {
        [] => None,
        [(owner, 1)] => Some(format!("1 repository from {owner} is now available")),
        [(owner, n)] => Some(format!("{n} repositories from {owner} are now available")),
        _ if total == 1 => Some("1 repository is now available".into()),
        _ => Some(format!("{total} repositories are now available")),
    }
}

/// The palette surface follows the app's command palette: a neutral card
/// whose input and footer sit on a faintly lifted layer around an inset
/// results panel.
mod look {
    use super::*;
    pub fn chrome() -> Hsla {
        palette::tone(hsla(0., 0., 0.122, 1.), hsla(0., 0., 0.975, 1.))
    }
    pub fn surface() -> Hsla {
        palette::tone(hsla(0., 0., 0.098, 1.), white())
    }
    pub fn border() -> Hsla {
        palette::tone(hsla(0., 0., 1., 0.07), hsla(0., 0., 0., 0.08))
    }
    pub fn wash() -> Hsla {
        palette::tone(hsla(0., 0., 1., 0.05), hsla(0., 0., 0., 0.045))
    }
    pub fn muted() -> Hsla {
        palette::rgb(MUTED).into()
    }
    pub fn cap(child: impl IntoElement) -> Div {
        div()
            .h(px(20.))
            .min_w(px(20.))
            .px(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .bg(wash())
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(muted())
            .child(child)
    }
    pub fn hint(key: impl IntoElement, label: &'static str) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(11.))
            .text_color(muted())
            .child(cap(key))
            .child(label)
    }
    pub fn group_label(label: impl Into<SharedString>) -> Div {
        div()
            .px(px(8.))
            .pt(px(8.))
            .pb(px(4.))
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(muted())
            .child(label.into())
    }
    pub fn row(id: impl Into<ElementId>, active: bool) -> Stateful<Div> {
        div()
            .id(id)
            .h(px(36.))
            .px(px(8.))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .rounded(px(8.))
            .text_size(px(13.))
            .text_color(palette::rgb(TEXT))
            .when(active, |el| el.bg(wash()))
    }
    pub fn checkbox(checked: bool, disabled: bool) -> Div {
        div()
            .flex_none()
            .size(px(16.))
            .rounded(px(4.))
            .border_1()
            .flex()
            .items_center()
            .justify_center()
            .map(|v| {
                if checked && disabled {
                    // Already connected: a quiet check, not a choice.
                    v.border_color(palette::rgb(BORDER))
                        .child(icon("check").size(px(11.)).text_color(muted()))
                } else if checked {
                    v.border_color(palette::rgb(TEXT).opacity(0.9))
                        .bg(palette::rgb(TEXT).opacity(0.9))
                        .child(icon("check").size(px(11.)).text_color(surface()))
                } else {
                    v.border_color(palette::rgb(BORDER)).bg(wash())
                }
            })
    }
}

impl RepositoryPanel {
    /// Opens the palette fresh: empty search, nothing selected, the first
    /// repository highlighted, and the catalog refreshed behind it.
    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_manage() || !self.github_connected() {
            return;
        }
        self.picker = true;
        self.selected.clear();
        self.palette_active = None;
        self.error = None;
        self.search
            .update(cx, |s, cx| s.set_value(String::new(), cx));
        self.refresh_repositories(cx);
        window.focus(&self.search.focus_handle(cx), cx);
        cx.notify();
    }

    pub(super) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.picker {
            return;
        }
        self.picker = false;
        self.fresh_repos.clear();
        if self.adding.is_empty() && self.queue.is_empty() {
            self.machine.setup_after_add = false;
        }
        window.focus(&self.picker_trigger, cx);
        cx.notify();
    }

    /// Reloads repositories and GitHub access only.
    pub(super) fn refresh_repositories(&mut self, cx: &mut Context<Self>) {
        if self.repository_busy() || self.owner.is_none() {
            return;
        }
        self.call("list", methods::CLOUD_PROJECTS, json!({}), cx);
        self.call("github", methods::CLOUD_GITHUB, json!({}), cx);
        cx.notify();
    }

    /// Repositories already in Cloud or on their way, by lowercase name.
    fn connected_repositories(&self) -> BTreeSet<String> {
        self.projects
            .iter()
            .map(|p| {
                p.repository_identity
                    .trim_start_matches("github.com/")
                    .to_lowercase()
            })
            .chain(self.adding.iter().cloned())
            .collect()
    }

    fn palette_model(&self, cx: &App) -> (Vec<Group>, Vec<Item>, Vec<Item>) {
        let query = self.search.read(cx).value(cx);
        let groups = self
            .github
            .as_ref()
            .map(|g| groups(g, &self.connected_repositories(), &self.fresh_repos, &query))
            .unwrap_or_default();
        let actions = actions(
            self.github
                .as_ref()
                .map(|g| g.installations.as_slice())
                .unwrap_or_default(),
            self.can_manage(),
        );
        let items = navigable(&groups, &actions);
        (groups, actions, items)
    }

    /// Queues `names` for adding and closes the palette. Rows appear at
    /// once as "Adding…" and settle as each connect returns.
    fn add_repositories(
        &mut self,
        names: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.repository_busy() || names.is_empty() || !self.can_manage() {
            return;
        }
        let wanted: BTreeSet<String> = names.iter().map(|n| n.to_lowercase()).collect();
        let mut queue: Vec<GithubRepository> = self
            .github
            .as_ref()
            .map(|g| {
                g.repositories
                    .iter()
                    .filter(|r| wanted.contains(&r.name_with_owner.to_lowercase()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        // `connect_next` pops from the back: keep the list's order.
        queue.sort_by_key(|r| {
            std::cmp::Reverse(
                names
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(&r.name_with_owner)),
            )
        });
        self.adding = queue
            .iter()
            .map(|r| r.name_with_owner.to_lowercase())
            .collect();
        self.queue = queue;
        self.close_palette(window, cx);
        self.selected.clear();
        self.error = None;
        self.notice = None;
        self.connect_next(cx);
        cx.notify();
    }

    fn run_palette_item(&mut self, item: &Item, window: &mut Window, cx: &mut Context<Self>) {
        match item {
            Item::Repo(name) => self.toggle_choice(name.clone(), cx),
            Item::Manage(id) => {
                if let Some(install) = self
                    .github
                    .as_ref()
                    .and_then(|g| g.installations.iter().find(|i| i.installation_id == *id))
                {
                    cx.open_url(&installation_url(install));
                }
            }
            Item::Install => {
                if !self.repository_busy() && self.can_manage() {
                    // The GitHub section below walks through the rest.
                    self.close_palette(window, cx);
                    self.call(
                        "github-install",
                        methods::CLOUD_GITHUB_INSTALL,
                        json!({}),
                        cx,
                    );
                }
            }
            Item::Refresh => self.refresh_repositories(cx),
        }
    }

    fn toggle_choice(&mut self, name: String, cx: &mut Context<Self>) {
        if !self.selected.remove(&name) {
            self.selected.insert(name.clone());
        }
        self.palette_active = Some(Item::Repo(name));
        cx.notify();
    }

    fn palette_key(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let (_, _, items) = self.palette_model(cx);
        let active = self
            .palette_active
            .clone()
            .filter(|a| items.contains(a))
            .or_else(|| items.first().cloned());
        match on_key(key, &items, active.as_ref(), &self.selected) {
            Outcome::Move(item) => {
                self.palette_active = Some(item);
                self.palette_reveal = true;
                cx.notify();
            }
            Outcome::Toggle(name) => self.toggle_choice(name, cx),
            Outcome::Add(names) => self.add_repositories(names, window, cx),
            Outcome::Run(item) => self.run_palette_item(&item, window, cx),
            Outcome::Close => self.close_palette(window, cx),
            Outcome::Pass => return false,
            Outcome::Nothing => {}
        }
        true
    }

    /// The trigger: the dithered primary "Add repository".
    pub(super) fn add_action(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.repository_busy();
        let disabled = busy || !self.github_connected() || !self.can_manage();
        let label = if self.projects.is_empty() && self.adding.is_empty() {
            "Add your first repository"
        } else {
            "Add repository"
        };
        kit::primary_button("add-cloud-repositories", label, !disabled)
            .track_focus(&self.picker_trigger.clone().tab_stop(!disabled))
            .when(!disabled, |b| {
                b.on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx)))
            })
            .into_any_element()
    }

    /// The open palette over a quieted window.
    pub(super) fn render_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let viewport = window.viewport_size();
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        let (groups, actions, items) = self.palette_model(cx);
        if self
            .palette_active
            .as_ref()
            .is_none_or(|a| !items.contains(a))
        {
            self.palette_active = items.first().cloned();
        }
        let active = self.palette_active.clone();
        let loading = self.pending.values().any(|p| p.kind == "github");
        let busy = self.repository_busy();

        let top = (vh * 0.14).clamp(16., 140.);
        let list_max = (vh - top - 50. - 46. - 48.).clamp(160., 440.);
        let mut list = column()
            .id("cloud-repository-palette-list")
            .min_h_0()
            .max_h(px(list_max))
            .overflow_y_scroll()
            .track_scroll(&self.palette_scroll)
            .p(px(8.));
        // Child index of each navigable row, to scroll the highlight into view.
        let mut child = 0usize;
        let mut reveal: Option<usize> = None;
        let mut push = |list: Stateful<Div>, el: AnyElement, item: Option<&Item>| {
            if item.is_some() && item == active.as_ref() {
                reveal = Some(child);
            }
            child += 1;
            list.child(el)
        };
        let has_repos = groups.iter().any(|g| !g.choices.is_empty());
        if !has_repos {
            let text = if loading && self.github.is_none() {
                "Loading repositories…"
            } else if self
                .github
                .as_ref()
                .is_some_and(|g| g.repositories.is_empty())
            {
                "No repositories yet"
            } else {
                "No matches"
            };
            list = push(
                list,
                div()
                    .py(px(20.))
                    .flex()
                    .justify_center()
                    .text_size(px(12.))
                    .text_color(look::muted())
                    .child(text)
                    .into_any_element(),
                None,
            );
        }
        for group in groups {
            let label = if group.fresh > 0 {
                format!("{} · {} new", group.owner, group.fresh)
            } else {
                group.owner.clone()
            };
            list = push(list, look::group_label(label).into_any_element(), None);
            for choice in group.choices {
                let item = Item::Repo(choice.name.clone());
                let is_active = active.as_ref() == Some(&item);
                let selected = self.selected.contains(&choice.name);
                let adding = self.adding.contains(&choice.name.to_lowercase());
                let name = choice.name.clone();
                let label = StyledText::new(SharedString::from(choice.name.clone()))
                    .with_highlights(choice.matched.iter().map(|r| {
                        (
                            r.clone(),
                            HighlightStyle {
                                font_weight: Some(FontWeight::SEMIBOLD),
                                ..Default::default()
                            },
                        )
                    }));
                let trailing: SharedString = if choice.connected {
                    if adding { "Adding…" } else { "Connected" }.into()
                } else {
                    ago(&choice.updated_at).unwrap_or_default().into()
                };
                let row = look::row(
                    SharedString::from(format!("cloud-repository-choice-{name}")),
                    is_active && !choice.connected,
                )
                .role(Role::Button)
                .aria_label(SharedString::from(name.clone()))
                .aria_selected(choice.connected || selected)
                .child(avatar(&choice.name, choice.avatar.as_deref(), 18.))
                .child(div().flex_1().min_w_0().truncate().child(label))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(look::muted())
                        .child(trailing),
                )
                .child(look::checkbox(
                    choice.connected || selected,
                    choice.connected,
                ))
                .map(|row| {
                    if choice.connected {
                        row.opacity(0.55)
                    } else {
                        let hover = item.clone();
                        row.cursor_pointer()
                            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                if *hovered && this.palette_active.as_ref() != Some(&hover) {
                                    this.palette_active = Some(hover.clone());
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_choice(name.clone(), cx)
                            }))
                    }
                });
                list = push(list, row.into_any_element(), Some(&item));
            }
        }
        list = push(list, look::group_label("GitHub").into_any_element(), None);
        let installations = self
            .github
            .as_ref()
            .map(|g| g.installations.clone())
            .unwrap_or_default();
        let several = actions
            .iter()
            .filter(|a| matches!(a, Item::Manage(_)))
            .count()
            > 1;
        for item in actions {
            let is_active = active.as_ref() == Some(&item);
            let (id, lead, label, external): (SharedString, AnyElement, SharedString, bool) =
                match &item {
                    Item::Manage(installation) => {
                        let install = installations
                            .iter()
                            .find(|i| i.installation_id == *installation);
                        let login = install.map(|i| i.account_login.clone()).unwrap_or_default();
                        (
                            format!("cloud-palette-manage-{installation}").into(),
                            if several {
                                avatar(&login, install.and_then(|i| i.avatar_url.as_deref()), 16.)
                            } else {
                                icon("site-github")
                                    .size(px(15.))
                                    .text_color(look::muted())
                                    .into_any_element()
                            },
                            if several {
                                format!("Manage repository access for {login}").into()
                            } else {
                                "Manage repository access".into()
                            },
                            true,
                        )
                    }
                    Item::Install => (
                        "cloud-palette-install".into(),
                        icon("site-github")
                            .size(px(15.))
                            .text_color(look::muted())
                            .into_any_element(),
                        "Install on another GitHub account".into(),
                        true,
                    ),
                    Item::Refresh => (
                        "cloud-palette-refresh".into(),
                        if loading {
                            icon("refresh")
                                .size(px(15.))
                                .text_color(look::muted())
                                .with_animation(
                                    "cloud-palette-refreshing",
                                    Animation::new(std::time::Duration::from_millis(900)).repeat(),
                                    |svg, t| {
                                        svg.with_transformation(Transformation::rotate(percentage(
                                            t,
                                        )))
                                    },
                                )
                                .into_any_element()
                        } else {
                            icon("refresh")
                                .size(px(15.))
                                .text_color(look::muted())
                                .into_any_element()
                        },
                        "Refresh repositories".into(),
                        false,
                    ),
                    Item::Repo(_) => continue,
                };
            let disabled = matches!(item, Item::Refresh | Item::Install) && busy;
            let run = item.clone();
            let hover = item.clone();
            let row = look::row(id, is_active)
                .role(Role::Button)
                .aria_label(label.clone())
                .child(
                    div()
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(lead),
                )
                .child(div().flex_1().min_w_0().truncate().child(label))
                .when(external, |row| {
                    row.child(
                        icon("arrow-up-right")
                            .size(px(13.))
                            .text_color(look::muted()),
                    )
                })
                .when(disabled, |row| row.opacity(0.5))
                .when(!disabled, |row| {
                    row.cursor_pointer()
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered && this.palette_active.as_ref() != Some(&hover) {
                                this.palette_active = Some(hover.clone());
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.run_palette_item(&run, window, cx)
                        }))
                });
            list = push(list, row.into_any_element(), Some(&item));
        }
        if std::mem::take(&mut self.palette_reveal)
            && let Some(ix) = reveal
        {
            self.palette_scroll.scroll_to_item(ix);
        }

        let count = self.selected.len();
        let can_add = count > 0 && !busy;
        let footer = row()
            .flex_none()
            .h(px(46.))
            .px(px(16.))
            .justify_between()
            .gap(px(12.))
            .border_t_1()
            .border_color(look::border())
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(look::muted())
                    .when(count > 0, |v| v.child(format!("{count} selected"))),
            )
            .child(
                row()
                    .gap(px(14.))
                    .child(look::hint("␣", "Toggle"))
                    .child(look::hint(
                        icon("return").size(px(12.)).text_color(look::muted()),
                        "Add",
                    ))
                    .child(look::hint("Esc", "Close"))
                    .when(count > 0, |v| {
                        v.child(
                            kit::primary_button(
                                "connect-selected-repositories",
                                format!("Add {count}"),
                                can_add,
                            )
                            .when(can_add, |b| {
                                b.on_click(cx.listener(|this, _, window, cx| {
                                    this.palette_key(Key::Submit, window, cx);
                                }))
                            }),
                        )
                    }),
            );

        let header = row()
            .flex_none()
            .h(px(50.))
            .px(px(16.))
            .gap(px(10.))
            .child(
                icon("magnifer")
                    .size(px(16.))
                    .flex_none()
                    .text_color(look::muted()),
            )
            .child(div().flex_1().min_w_0().child(self.search.clone()));

        let card = column()
            .id("cloud-repository-picker")
            .role(Role::Dialog)
            .aria_label("Add repositories")
            .key_context("CloudRepositoryPicker")
            .track_focus(&self.picker_focus)
            .w(px(576_f32.min(vw - 32.)))
            .rounded(px(16.))
            .border_1()
            .border_color(look::border())
            .bg(look::chrome())
            .shadow_lg()
            .overflow_hidden()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|this, _: &CloseRepositoryPicker, window, cx| {
                this.close_palette(window, cx);
                cx.stop_propagation();
            }))
            .capture_action(
                cx.listener(|this, _: &gpui_base::input::MoveUp, window, cx| {
                    this.palette_key(Key::Up, window, cx);
                    cx.stop_propagation();
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_base::input::MoveDown, window, cx| {
                    this.palette_key(Key::Down, window, cx);
                    cx.stop_propagation();
                }),
            )
            .capture_action(
                cx.listener(|this, e: &gpui_base::input::Enter, window, cx| {
                    if !e.shift && !e.secondary {
                        this.palette_key(Key::Submit, window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_base::input::Escape, window, cx| {
                    this.palette_key(Key::Close, window, cx);
                    cx.stop_propagation();
                }),
            )
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                let plain = !e.keystroke.modifiers.modified();
                let key = match e.keystroke.key.as_str() {
                    "space" if plain => Key::Toggle,
                    "escape" => Key::Close,
                    "up" if plain => Key::Up,
                    "down" if plain => Key::Down,
                    _ => return,
                };
                if this.palette_key(key, window, cx) {
                    cx.stop_propagation();
                    window.prevent_default();
                }
            }))
            .child(header)
            .child(
                div()
                    .mx(px(-1.))
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .rounded_t(px(12.))
                    .border_1()
                    .border_b_0()
                    .border_color(look::border())
                    .bg(look::surface())
                    .child(list),
            )
            .child(footer);

        deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .id("cloud-repository-palette-scrim")
                    .occlude()
                    .w(viewport.width)
                    .h(viewport.height)
                    .bg(palette::tone(
                        hsla(0., 0., 0., 0.45),
                        hsla(0., 0., 0., 0.18),
                    ))
                    .flex()
                    .flex_col()
                    .items_center()
                    .pt(px(top))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.close_palette(window, cx)),
                    )
                    .child(card),
            ),
        )
        .priority(2)
        .into_any_element()
    }

    /// Notes that a GitHub catalog arrived: repositories that weren't there
    /// before are marked new and announced in a toast.
    pub(super) fn note_catalog(&mut self, catalog: &GithubCatalog, cx: &mut Context<Self>) {
        let names: BTreeSet<String> = catalog
            .repositories
            .iter()
            .map(|r| r.name_with_owner.to_lowercase())
            .collect();
        let connected = self.connected_repositories();
        if let Some(known) = &self.known_repos {
            let mut arrivals: Vec<(String, usize)> = vec![];
            for repo in &catalog.repositories {
                let lower = repo.name_with_owner.to_lowercase();
                // Only repositories you can still add are news.
                if known.contains(&lower) || connected.contains(&lower) {
                    continue;
                }
                self.fresh_repos.insert(lower);
                let owner = owner_of(&repo.name_with_owner);
                match arrivals
                    .iter_mut()
                    .find(|(o, _)| o.eq_ignore_ascii_case(owner))
                {
                    Some((_, n)) => *n += 1,
                    None => arrivals.push((owner.to_owned(), 1)),
                }
            }
            if let Some(message) = arrivals_message(&arrivals) {
                self.show_toast(message, cx);
            }
        }
        self.known_repos = Some(names);
    }

    fn show_toast(&mut self, message: String, cx: &mut Context<Self>) {
        self.toast = Some(message.into());
        self.toast_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(6))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.toast = None;
                this.toast_timer = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The small confirmation in the window's corner. Clicking it opens the
    /// palette on the new repositories.
    pub(super) fn render_toast(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let message = self.toast.clone()?;
        let viewport = window.viewport_size();
        let ok = palette::tone(
            hsla(133.9 / 360., 0.606, 0.488, 1.),
            hsla(131.4 / 360., 0.734, 0.31, 1.),
        );
        let toast = row()
            .id("cloud-repositories-toast")
            .role(Role::Status)
            .aria_label(message.clone())
            .w(px(340_f32.min(f32::from(viewport.width) - 32.)))
            .px(px(14.))
            .py(px(12.))
            .gap(px(10.))
            .items_start()
            .rounded(px(12.))
            .border_1()
            .border_color(look::border())
            .bg(look::chrome())
            .shadow_lg()
            .occlude()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| {
                this.toast = None;
                this.toast_timer = None;
                if !this.picker {
                    this.open_palette(window, cx);
                    // Keep the "new" marks this toast announced.
                }
                cx.notify();
            }))
            .child(
                icon("check-circle")
                    .size(px(16.))
                    .flex_none()
                    .mt(px(1.))
                    .text_color(ok),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.5))
                    .line_height(px(18.))
                    .text_color(palette::rgb(TEXT))
                    .child(message),
            )
            .child(
                div()
                    .id("dismiss-cloud-repositories-toast")
                    .role(Role::Button)
                    .aria_label("Dismiss")
                    .flex_none()
                    .size(px(18.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|v| v.bg(look::wash()))
                    .child(icon("close").size(px(12.)).text_color(look::muted()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.toast = None;
                        this.toast_timer = None;
                        cx.notify();
                    })),
            );
        Some(
            deferred(
                anchored()
                    .anchor(Anchor::BottomRight)
                    .position(point(viewport.width - px(20.), viewport.height - px(20.)))
                    .child(toast),
            )
            .priority(3)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    use zuse_protocol::cloud_projects::GithubInstallation;

    fn repo(name: &str, updated: &str) -> GithubRepository {
        GithubRepository {
            name_with_owner: name.into(),
            description: None,
            ssh_url: String::new(),
            https_url: format!("https://github.com/{name}"),
            is_private: false,
            default_branch: "main".into(),
            updated_at: updated.into(),
            owner_avatar_url: None,
        }
    }
    fn install(id: i64, login: &str) -> GithubInstallation {
        GithubInstallation {
            installation_id: id,
            account_login: login.into(),
            account_type: "User".into(),
            avatar_url: Some(format!("https://avatars.example/{login}")),
            repository_selection: "all".into(),
            suspended: false,
        }
    }
    fn catalog() -> GithubCatalog {
        GithubCatalog {
            configured: true,
            can_manage_installations: Some(true),
            user: None,
            installations: vec![install(1, "swarajbachu"), install(2, "opcodehq")],
            repositories: vec![
                repo("opcodehq/web", "2026-11-10T00:00:00Z"),
                repo("swarajbachu/spacekey", "2026-09-01T00:00:00Z"),
                repo("opcodehq/zuse", "2026-11-18T00:00:00Z"),
                repo("swarajbachu/zuse", "2026-11-17T00:00:00Z"),
                repo("opcodehq/zkit", "2026-11-16T00:00:00Z"),
            ],
        }
    }
    fn names(groups: &[Group]) -> Vec<(String, Vec<String>)> {
        groups
            .iter()
            .map(|g| {
                (
                    g.owner.clone(),
                    g.choices.iter().map(|c| c.name.clone()).collect(),
                )
            })
            .collect()
    }
    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn fuzzy_matches_in_order_and_prefers_substrings_in_the_repository() {
        assert!(fuzzy("zse", "opcodehq/zuse").is_some());
        assert!(fuzzy("esuz", "opcodehq/zuse").is_none());
        assert_eq!(fuzzy("", "a/b"), Some((0, vec![])));
        // Case and whitespace don't matter.
        assert_eq!(fuzzy("ZU SE", "opcodehq/zuse").unwrap().1, vec![9..13]);
        // The repository part beats the owner.
        let (in_repo, _) = fuzzy("zuse", "opcodehq/zuse").unwrap();
        let (in_owner, _) = fuzzy("zuse", "zuse/other").unwrap();
        assert!(in_repo > in_owner);
        // Contiguous beats scattered.
        let (exact, _) = fuzzy("web", "opcodehq/web").unwrap();
        let (scattered, ranges) = fuzzy("wb", "opcodehq/web").unwrap();
        assert!(exact > scattered);
        assert_eq!(ranges, vec![9..10, 11..12]);
    }

    #[test]
    fn groups_follow_installations_with_newest_first() {
        let groups = groups(&catalog(), &BTreeSet::new(), &BTreeSet::new(), "");
        assert_eq!(
            names(&groups),
            vec![
                (
                    "swarajbachu".into(),
                    vec!["swarajbachu/zuse".into(), "swarajbachu/spacekey".into()]
                ),
                (
                    "opcodehq".into(),
                    vec![
                        "opcodehq/zuse".into(),
                        "opcodehq/zkit".into(),
                        "opcodehq/web".into()
                    ]
                ),
            ]
        );
        // Repositories fall back to their account's avatar.
        assert_eq!(
            groups[0].choices[0].avatar.as_deref(),
            Some("https://avatars.example/swarajbachu")
        );
    }

    #[test]
    fn a_search_ranks_matches_within_each_account() {
        let groups = groups(&catalog(), &BTreeSet::new(), &BTreeSet::new(), "zk");
        assert_eq!(
            names(&groups),
            vec![("opcodehq".into(), vec!["opcodehq/zkit".into()])]
        );
        let groups = super::groups(&catalog(), &BTreeSet::new(), &BTreeSet::new(), "zu");
        assert_eq!(
            names(&groups)
                .into_iter()
                .map(|(_, n)| n)
                .collect::<Vec<_>>(),
            vec![
                vec!["swarajbachu/zuse".to_string()],
                vec!["opcodehq/zuse".into()]
            ]
        );
    }

    #[test]
    fn connected_repositories_show_checked_after_the_rest_and_are_skipped() {
        let connected = set(&["opcodehq/zuse"]);
        let groups = groups(&catalog(), &connected, &BTreeSet::new(), "");
        let opcode = &groups[1];
        assert_eq!(opcode.choices.last().unwrap().name, "opcodehq/zuse");
        assert!(opcode.choices.last().unwrap().connected);
        let items = navigable(&groups, &[Item::Refresh]);
        assert!(!items.contains(&Item::Repo("opcodehq/zuse".into())));
        assert_eq!(items.last(), Some(&Item::Refresh));
    }

    #[test]
    fn fresh_repositories_are_counted_per_account_unless_connected() {
        let fresh = set(&["opcodehq/zkit", "opcodehq/web", "opcodehq/zuse"]);
        let groups = groups(&catalog(), &set(&["opcodehq/zuse"]), &fresh, "");
        assert_eq!(groups[0].fresh, 0);
        assert_eq!(groups[1].fresh, 2);
    }

    #[test]
    fn actions_manage_each_live_installation_then_install_and_refresh() {
        let mut installs = vec![install(1, "a"), install(2, "b")];
        installs[1].suspended = true;
        assert_eq!(
            actions(&installs, true),
            vec![Item::Manage(1), Item::Install, Item::Refresh]
        );
        assert_eq!(
            actions(&installs, false),
            vec![Item::Manage(1), Item::Refresh]
        );
    }

    #[test]
    fn keys_move_toggle_add_and_close() {
        let a = Item::Repo("o/a".into());
        let b = Item::Repo("o/b".into());
        let items = vec![a.clone(), b.clone(), Item::Refresh];
        let none = BTreeSet::new();
        // Arrows wrap.
        assert_eq!(
            on_key(Key::Down, &items, Some(&a), &none),
            Outcome::Move(b.clone())
        );
        assert_eq!(
            on_key(Key::Down, &items, Some(&Item::Refresh), &none),
            Outcome::Move(a.clone())
        );
        assert_eq!(
            on_key(Key::Up, &items, Some(&a), &none),
            Outcome::Move(Item::Refresh)
        );
        assert_eq!(on_key(Key::Up, &[], None, &none), Outcome::Nothing);
        // Space toggles a repository; elsewhere it types.
        assert_eq!(
            on_key(Key::Toggle, &items, Some(&b), &none),
            Outcome::Toggle("o/b".into())
        );
        assert_eq!(
            on_key(Key::Toggle, &items, Some(&Item::Refresh), &none),
            Outcome::Pass
        );
        assert_eq!(on_key(Key::Toggle, &items, None, &none), Outcome::Pass);
        // Enter adds the highlighted repository, or the selection in list order.
        assert_eq!(
            on_key(Key::Submit, &items, Some(&a), &none),
            Outcome::Add(vec!["o/a".into()])
        );
        let picked = set(&["o/b", "o/a"]);
        assert_eq!(
            on_key(Key::Submit, &items, Some(&a), &picked),
            Outcome::Add(vec!["o/a".into(), "o/b".into()])
        );
        // Enter on an action runs it, even with a selection.
        assert_eq!(
            on_key(Key::Submit, &items, Some(&Item::Refresh), &picked),
            Outcome::Run(Item::Refresh)
        );
        assert_eq!(on_key(Key::Submit, &[], None, &none), Outcome::Nothing);
        assert_eq!(on_key(Key::Close, &items, Some(&a), &none), Outcome::Close);
    }

    #[test]
    fn arrivals_read_naturally() {
        assert_eq!(arrivals_message(&[]), None);
        assert_eq!(
            arrivals_message(&[("opcodehq".into(), 18)]).unwrap(),
            "18 repositories from opcodehq are now available"
        );
        assert_eq!(
            arrivals_message(&[("opcodehq".into(), 1)]).unwrap(),
            "1 repository from opcodehq is now available"
        );
        assert_eq!(
            arrivals_message(&[("a".into(), 2), ("b".into(), 3)]).unwrap(),
            "5 repositories are now available"
        );
    }

    fn open(
        cx: &mut TestAppContext,
        first: Option<GithubCatalog>,
    ) -> WindowHandle<RepositoryPanel> {
        cx.update(crate::initialize);
        let window = cx.add_window(RepositoryPanel::new);
        window
            .update(cx, |panel, window, cx| {
                panel.bind_owner(Some(("account".into(), None)), cx);
                panel.loaded = true;
                panel.page = CloudPage::Repositories;
                if let Some(first) = first {
                    panel.note_catalog(&first, cx);
                    assert!(panel.toast.is_none());
                    panel.note_catalog(&catalog(), cx);
                }
                panel.github = Some(catalog());
                panel.open_palette(window, cx);
                // Settle the palette's own refresh.
                panel.pending.clear();
            })
            .unwrap();
        window
    }

    #[gpui::test]
    fn arrows_space_and_enter_add_the_chosen_repositories(cx: &mut TestAppContext) {
        let window = open(cx, None);
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        cx.update(|window, cx| window.draw(cx).clear());
        cx.simulate_input("zu");
        cx.update(|window, cx| window.draw(cx).clear());
        // Space toggles the highlighted repository instead of typing.
        cx.simulate_keystrokes("space down space");
        cx.update(|window, cx| window.draw(cx).clear());
        window
            .update(cx, |panel, _, cx| {
                assert_eq!(panel.search.read(cx).value(cx), "zu");
                assert_eq!(panel.selected, set(&["swarajbachu/zuse", "opcodehq/zuse"]));
            })
            .unwrap();
        cx.simulate_keystrokes("enter");
        window
            .update(cx, |panel, _, _| {
                assert!(!panel.picker);
                assert!(panel.selected.is_empty());
                assert_eq!(panel.adding, set(&["swarajbachu/zuse", "opcodehq/zuse"]));
                let connects = panel.pending.values().filter(|p| p.kind == "connect");
                assert_eq!(connects.count(), 1);
            })
            .unwrap();
    }

    #[gpui::test]
    fn new_repositories_are_announced_and_escape_closes(cx: &mut TestAppContext) {
        let mut first = catalog();
        first.repositories.truncate(2);
        let window = open(cx, Some(first));
        window
            .update(cx, |panel, _, _| {
                assert_eq!(
                    panel.toast.as_deref(),
                    Some("3 repositories are now available")
                );
            })
            .unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        cx.update(|window, cx| window.draw(cx).clear());
        cx.simulate_keystrokes("escape");
        window
            .update(cx, |panel, _, _| {
                assert!(!panel.picker);
                assert!(panel.fresh_repos.is_empty());
            })
            .unwrap();
    }
}
