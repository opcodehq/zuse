//! The `MachineImageProposal` card of a setup chat: what the agent proposes
//! for the account's machine image (the setup script as a diff, repositories
//! added and removed) with Apply & rebuild and Discard, then the build.
//!
//! Like plugin cards, the spec only names the proposal; its content and the
//! build are read from the host's [`MachineSetupDirectory`] on every render,
//! and the buttons emit [`UiBlockEvent::MachineSetup`] for the host to call
//! the cloud.

use super::{UiBlockEvent, UiBlockView, eid, icon};
use gpui::{prelude::*, *};
use std::collections::HashMap;
use zuse_protocol::machine_setup::ProposalView;

/// What the host knows about the proposals cards show.
#[derive(Clone, Debug, Default)]
pub struct MachineSetupDirectory {
    pub proposals: HashMap<String, ProposalCard>,
    /// This host cannot read or apply proposals (e.g. a read-only view).
    pub unavailable: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ProposalCard {
    pub view: Option<ProposalView>,
    /// A request for this proposal is in flight.
    pub busy: bool,
    pub error: Option<String>,
}

impl PartialEq for MachineSetupDirectory {
    fn eq(&self, other: &Self) -> bool {
        self.unavailable == other.unavailable
            && self.proposals.len() == other.proposals.len()
            && self.proposals.iter().all(|(id, card)| {
                other.proposals.get(id).is_some_and(|o| {
                    o.busy == card.busy
                        && o.error == card.error
                        && o.view.as_ref().map(stamp) == card.view.as_ref().map(stamp)
                })
            })
    }
}

/// What a card's rendering depends on.
fn stamp(view: &ProposalView) -> (String, String, Option<String>, i64, i64) {
    let image = view.machine.image.as_ref();
    (
        view.proposal.state.clone(),
        view.machine.image_state.clone(),
        image.and_then(|i| i.phase.clone()),
        image.map(|i| i.updated_at).unwrap_or_default(),
        view.machine.revision,
    )
}

/// What a proposal card asks its host to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MachineSetupAction {
    Apply { proposal: String },
    Discard { proposal: String },
}

/// One line of a script diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLine {
    Same(String),
    Added(String),
    Removed(String),
}

/// Line diff of `before` → `after` (longest common subsequence). Very long
/// scripts fall back to "all removed, all added" rather than a slow table.
pub fn line_diff(before: &str, after: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    if a.len() * b.len() > 4_000_000 {
        return a
            .iter()
            .map(|l| DiffLine::Removed((*l).to_owned()))
            .chain(b.iter().map(|l| DiffLine::Added((*l).to_owned())))
            .collect();
    }
    let mut table = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            out.push(DiffLine::Same(a[i].to_owned()));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            out.push(DiffLine::Removed(a[i].to_owned()));
            i += 1;
        } else {
            out.push(DiffLine::Added(b[j].to_owned()));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| DiffLine::Removed((*l).to_owned())));
    out.extend(b[j..].iter().map(|l| DiffLine::Added((*l).to_owned())));
    out
}

/// Changed lines with up to two lines of context; gaps become `None`.
fn hunks(diff: &[DiffLine]) -> Vec<Option<&DiffLine>> {
    let near = |i: usize| {
        diff[i.saturating_sub(2)..(i + 3).min(diff.len())]
            .iter()
            .any(|l| !matches!(l, DiffLine::Same(_)))
    };
    let mut out = Vec::new();
    let mut gap = false;
    for (i, line) in diff.iter().enumerate() {
        if near(i) {
            if gap && !out.is_empty() {
                out.push(None);
            }
            gap = false;
            out.push(Some(line));
        } else {
            gap = true;
        }
    }
    out
}

/// The card's headline state.
pub fn status(view: &ProposalView) -> (&'static str, Option<&'static str>) {
    let machine = &view.machine;
    match view.proposal.state.as_str() {
        "proposed" => ("Proposed change", None),
        "discarded" => ("Discarded", None),
        "superseded" => ("Replaced by a newer proposal", None),
        _ => match machine.image_state.as_str() {
            "queued" => ("Applied", Some("Waiting to build")),
            "building" => (
                "Applied",
                Some(crate::cloud_repositories::phase_label(
                    machine
                        .image
                        .as_ref()
                        .and_then(|i| i.phase.as_deref())
                        .unwrap_or_default(),
                )),
            ),
            "ready" => ("Applied", Some("Image ready")),
            "failed" => ("Applied", Some("Build failed")),
            _ => ("Applied", None),
        },
    }
}

const MAX_DIFF_LINES: usize = 60;

impl UiBlockView {
    /// Whether the spec shows a proposal card, so the host only feeds
    /// [`Self::set_machine_setup`] to blocks that need it.
    pub fn machine_setup_proposals(&self) -> Vec<String> {
        fn collect(node: &zuse_ui_block::Component, out: &mut Vec<String>) {
            if let zuse_ui_block::Component::MachineImageProposal { proposal } = node {
                out.push(proposal.clone());
            }
            for child in node.children() {
                collect(child, out);
            }
        }
        let mut out = Vec::new();
        if let Ok(root) = &self.root {
            collect(root, &mut out);
        }
        out
    }

    /// The host's proposals; re-renders only on change.
    pub fn set_machine_setup(
        &mut self,
        directory: std::rc::Rc<MachineSetupDirectory>,
        cx: &mut Context<Self>,
    ) {
        if *self.machine_setup != *directory {
            self.machine_setup = directory;
            cx.notify();
        }
    }

    fn setup_button(
        &self,
        path: &str,
        key: &str,
        label: &str,
        primary: bool,
        enabled: bool,
        action: MachineSetupAction,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = eid(path, key);
        let button = if primary {
            crate::styling::dither_action(id, label.to_owned(), enabled, true)
        } else {
            let p = &self.palette;
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(SharedString::from(label.to_owned()))
                .h(px(28.))
                .px_3()
                .flex()
                .items_center()
                .text_size(px(12.))
                .text_color(p.muted_foreground)
                .when(!enabled, |d| d.opacity(0.45))
                .when(enabled, |d| {
                    let fg = p.foreground;
                    d.cursor_pointer().hover(move |s| s.text_color(fg))
                })
                .child(label.to_owned())
        };
        button
            .when(enabled, |b| {
                b.tab_index(0).on_click(cx.listener(move |this, _, _, cx| {
                    if this.plugin_actions {
                        cx.emit(UiBlockEvent::MachineSetup(action.clone()));
                    }
                }))
            })
            .into_any_element()
    }

    /// A `MachineImageProposal(id)` card.
    pub(super) fn machine_image_card(
        &mut self,
        id: &str,
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.clone();
        let card = self.machine_setup.proposals.get(id).cloned();
        let frame = div()
            .id(eid(path, "machine-image"))
            .role(Role::Group)
            .aria_label(SharedString::from("Machine image change"))
            .flex()
            .flex_col()
            .gap(px(12.))
            .min_w_0()
            .px(px(16.))
            .py(px(14.))
            .rounded(px(18.))
            .border_1()
            .border_color(p.border)
            .bg(p.card);
        let muted = |text: String| {
            div()
                .text_size(px(12.))
                .line_height(px(17.))
                .text_color(p.muted_foreground)
                .child(text)
        };
        let Some(view) = card.as_ref().and_then(|c| c.view.clone()) else {
            let line = if self.machine_setup.unavailable {
                "Open this chat in the Zuse app to review the image change.".to_owned()
            } else if let Some(error) = card.as_ref().and_then(|c| c.error.clone()) {
                error
            } else {
                "Loading the proposed image change…".to_owned()
            };
            return frame
                .child(header(&p, "Machine image change", None))
                .child(muted(line))
                .into_any_element();
        };
        let busy = card.as_ref().is_some_and(|c| c.busy);
        let error = card.as_ref().and_then(|c| c.error.clone());
        let proposal = &view.proposal;
        let (title, detail) = status(&view);
        let mut body = frame.child(header(&p, title, detail));
        if !proposal.notes.trim().is_empty() {
            body = body.child(
                div()
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .text_color(p.foreground)
                    .child(proposal.notes.clone()),
            );
        }
        // Setup script.
        let diff = line_diff(&proposal.current_setup_script, &proposal.setup_script);
        // Changed lines with context; "Show full script" lists every line
        // so the owner can read all of it before applying.
        let full = self.machine_setup_full.contains(path);
        let shown: Vec<Option<&DiffLine>> = if full {
            diff.iter().map(Some).collect()
        } else {
            hunks(&diff)
        };
        let limit = if full { usize::MAX } else { MAX_DIFF_LINES };
        let mut script = div()
            .flex()
            .flex_col()
            .min_w_0()
            .p(px(10.))
            .rounded(px(10.))
            .bg(p.muted.opacity(0.4))
            .font_family(p.font_mono.clone())
            .text_size(px(11.5))
            .line_height(px(16.));
        if shown.is_empty() {
            script = script.child(muted("Setup script unchanged".into()));
        }
        for line in shown.iter().take(limit) {
            let (mark, text, color, bg) = match line {
                None => ("", "⋯".to_owned(), p.muted_foreground, None),
                Some(DiffLine::Same(t)) => (" ", t.clone(), p.muted_foreground, None),
                Some(DiffLine::Added(t)) => {
                    ("+", t.clone(), p.foreground, Some(p.alert_success_bg))
                }
                Some(DiffLine::Removed(t)) => {
                    ("−", t.clone(), p.muted_foreground, Some(p.alert_error_bg))
                }
            };
            script = script.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .px(px(4.))
                    .rounded(px(3.))
                    .when_some(bg, |d, bg| d.bg(bg))
                    .text_color(color)
                    .child(div().w(px(8.)).flex_none().child(mark))
                    // Long commands wrap rather than hide their end.
                    .child(div().flex_1().min_w_0().child(text)),
            );
        }
        let hidden = shown.len().saturating_sub(limit);
        let toggle = (full || hidden > 0 || shown.len() < diff.len()).then(|| {
            let fg = p.foreground;
            let key = path.to_owned();
            div()
                .id(eid(path, "full-script"))
                .role(Role::Button)
                .tab_index(0)
                .text_size(px(12.))
                .text_color(p.muted_foreground)
                .cursor_pointer()
                .hover(move |s| s.text_color(fg))
                .child(if full {
                    "Show changes only".to_owned()
                } else if hidden > 0 {
                    format!("Show full script · {hidden} more changed lines")
                } else {
                    "Show full script".to_owned()
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.machine_setup_full.remove(&key) {
                        this.machine_setup_full.insert(key.clone());
                    }
                    cx.notify();
                }))
        });
        body = body
            .child(label(&p, "Setup script"))
            .child(script)
            .children(toggle);
        // Repositories.
        let (added, removed) = proposal.repository_changes();
        let mut repos = div().flex().flex_col().gap(px(4.));
        if added.is_empty() && removed.is_empty() {
            repos = repos.child(muted(format!(
                "Unchanged · {}",
                plural(proposal.repositories.len(), "repository", "repositories")
            )));
        }
        for (mark, name, color) in added
            .iter()
            .map(|r| ("+", *r, p.success))
            .chain(removed.iter().map(|r| ("−", *r, p.destructive)))
        {
            repos = repos.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.5))
                    .child(div().w(px(10.)).text_color(color).child(mark))
                    .child(
                        div()
                            .text_color(p.foreground)
                            .child(name.trim_start_matches("github.com/").to_owned()),
                    ),
            );
        }
        body = body.child(label(&p, "Repositories")).child(repos);
        // After applying: what the build did.
        if proposal.state == "applied" {
            body = body.child(self.build_section(&view));
        }
        if let Some(error) = error {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(p.destructive)
                    .child(error),
            );
        }
        if proposal.state == "proposed" {
            let enabled = !busy && self.plugin_actions && !self.machine_setup.unavailable;
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(6.))
                    .child(self.setup_button(
                        path,
                        "discard",
                        "Discard",
                        false,
                        enabled,
                        MachineSetupAction::Discard {
                            proposal: id.to_owned(),
                        },
                        cx,
                    ))
                    .child(self.setup_button(
                        path,
                        "apply",
                        if busy {
                            "Applying…"
                        } else {
                            "Apply & rebuild"
                        },
                        true,
                        enabled,
                        MachineSetupAction::Apply {
                            proposal: id.to_owned(),
                        },
                        cx,
                    )),
            );
        }
        body.into_any_element()
    }

    fn build_section(&self, view: &ProposalView) -> Div {
        let p = &self.palette;
        let machine = &view.machine;
        let mut out = div().flex().flex_col().gap(px(6.));
        let image = machine.image.as_ref();
        if machine.image_state == "building" {
            let fraction = crate::cloud_repositories::phase_fraction(
                image.and_then(|i| i.phase.as_deref()).unwrap_or_default(),
            );
            out = out.child(crate::cloud_settings::kit::meter(fraction, 6.));
        }
        let line = match machine.image_state.as_str() {
            "queued" | "building" => {
                "Computers keep using your previous image until this build is verified."
            }
            "ready" => "New computers start from this image.",
            "failed" => "Computers keep using your last working image.",
            _ => "",
        };
        if !line.is_empty() {
            out = out.child(
                div()
                    .text_size(px(12.))
                    .text_color(p.muted_foreground)
                    .child(line),
            );
        }
        if let Some(image) = image.filter(|i| i.state == "ready" || i.state == "failed") {
            for repo in &image.repositories {
                let (glyph, color, note) = if repo.state == "baked" {
                    ("check-circle", p.success, String::new())
                } else {
                    (
                        "info-circle",
                        p.warning,
                        repo.reason.clone().unwrap_or_default(),
                    )
                };
                out = out.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(12.))
                        .child(icon(glyph, 12., color))
                        .child(
                            div().text_color(p.foreground).child(
                                repo.repository_identity
                                    .trim_start_matches("github.com/")
                                    .to_owned(),
                            ),
                        )
                        .when(!note.is_empty(), |d| {
                            d.child(div().text_color(p.muted_foreground).child(note))
                        }),
                );
            }
            if let Some(log) = image.log_excerpt.clone().filter(|l| !l.is_empty()) {
                out = out.child(
                    div()
                        .max_h(px(160.))
                        .overflow_hidden()
                        .p(px(8.))
                        .rounded(px(8.))
                        .bg(p.muted.opacity(0.4))
                        .font_family(p.font_mono.clone())
                        .text_size(px(11.))
                        .text_color(p.muted_foreground)
                        .child(log),
                );
            }
        }
        out
    }
}

fn header(p: &super::UiBlockPalette, title: &str, detail: Option<&str>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(icon("dither-monitor", 16., p.foreground))
        .child(
            div()
                .text_size(px(15.))
                .line_height(px(20.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.foreground)
                .child(title.to_owned()),
        )
        .when_some(detail, |d, detail| {
            d.child(
                div()
                    .text_size(px(12.))
                    .text_color(p.muted_foreground)
                    .child(format!("· {detail}")),
            )
        })
}

fn label(p: &super::UiBlockPalette, text: &str) -> Div {
    div()
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.muted_foreground)
        .child(text.to_owned())
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn script_diffs_keep_order_and_show_only_changes_with_context() {
        let diff = line_diff("a\nb\nc\n", "a\nx\nc\nd\n");
        assert_eq!(
            diff,
            vec![
                DiffLine::Same("a".into()),
                DiffLine::Removed("b".into()),
                DiffLine::Added("x".into()),
                DiffLine::Same("c".into()),
                DiffLine::Added("d".into()),
            ]
        );
        let long: String = (0..20).map(|i| format!("l{i}\n")).collect();
        let changed = long.replace("l10\n", "l10!\n");
        let diff = line_diff(&long, &changed);
        let shown = hunks(&diff);
        assert_eq!(shown.len(), 6, "two lines of context each side");
        assert!(hunks(&line_diff("same", "same")).is_empty());
        let fresh = line_diff("", "set -euo pipefail\ncurl x");
        assert!(fresh.iter().all(|l| matches!(l, DiffLine::Added(_))));
    }

    #[test]
    fn status_follows_the_proposal_then_the_build() {
        let mut view = ProposalView {
            proposal: zuse_protocol::machine_setup::Proposal {
                state: "proposed".into(),
                ..Default::default()
            },
            machine: serde_json::from_value(serde_json::json!({
                "revision":1,"recipe":{},"createdAt":0,"imageState":"ready"
            }))
            .unwrap(),
        };
        assert_eq!(status(&view), ("Proposed change", None));
        view.proposal.state = "applied".into();
        view.machine.image_state = "building".into();
        view.machine.image = Some(zuse_protocol::cloud_computers::MachineImageBuild {
            state: "building".into(),
            phase: Some("setup".into()),
            ..Default::default()
        });
        assert_eq!(
            status(&view),
            ("Applied", Some("Running your setup script"))
        );
        view.machine.image_state = "failed".into();
        assert_eq!(status(&view).1, Some("Build failed"));
    }
}
