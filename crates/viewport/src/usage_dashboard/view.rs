use super::*;
use crate::{
    assets::icon,
    palette::{self, rgb},
    settings_style as style,
    shell::{BG, BORDER, MUTED, TEXT},
};
use zuse_protocol::{
    HarnessId,
    usage::{CostStatus, Group},
};

pub(super) fn row() -> Div {
    div().flex().items_center()
}
pub(super) fn column() -> Div {
    div().flex().flex_col()
}
pub(super) fn label(text: impl Into<SharedString>, size: f32) -> Div {
    div()
        .text_size(px(size))
        .line_height(px(size * 1.35))
        .child(text.into())
}
fn frame() -> Div {
    column()
        .flex_none()
        .rounded(px(12.))
        .p(px(4.))
        .bg(style::frame_bg())
}
fn card() -> Div {
    column()
        .rounded(px(10.))
        .border_1()
        .border_color(palette::tone(
            gpui::rgb(0xffffff).opacity(0.08).into(),
            gpui::rgb(0xe1e7df).into(),
        ))
        .bg(style::card_bg())
        .min_w_0()
}
pub(super) fn tokens(value: u64) -> String {
    if value >= 1_000_000 {
        trim(value as f64 / 1_000_000., "M")
    } else if value >= 1000 {
        trim(value as f64 / 1000., "K")
    } else {
        value.to_string()
    }
}
fn trim(value: f64, suffix: &str) -> String {
    let v = format!("{value:.1}");
    format!("{}{suffix}", v.trim_end_matches('0').trim_end_matches('.'))
}
pub(super) fn money(value: Option<f64>) -> String {
    value
        .map(|v| format!("${v:.2}"))
        .unwrap_or_else(|| "—".into())
}
fn delta(value: Option<f64>, previous: Option<f64>) -> Option<String> {
    let (v, p) = (value?, previous?);
    if p == 0. {
        return None;
    }
    let d = ((v - p) / p * 100.).round();
    Some(format!("{}{d:.0}% vs prior", if d > 0. { "+" } else { "" }))
}
fn date(group: &Group) -> String {
    group
        .ended_at
        .or(group.started_at)
        .map(|d| d.with_timezone(&Local).format("%b %-d").to_string())
        .unwrap_or_else(|| "—".into())
}
const PROVIDERS: [(HarnessId, &str, &str, &str); 5] = [
    (
        HarnessId::ClaudeCode,
        "claude",
        "Claude Code",
        "claude-mark",
    ),
    (HarnessId::Codex, "codex", "Codex", "openai-mark"),
    (HarnessId::Grok, "grok", "Grok", "grok-mark"),
    (HarnessId::Gemini, "gemini", "Gemini", "gemini-mark"),
    (HarnessId::Kiro, "kiro", "Kiro", "kiro-mark"),
];
impl UsageDashboard {
    fn button(
        &mut self,
        id: impl Into<String>,
        text: impl Into<SharedString>,
        selected: bool,
        action: Action,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = id.into();
        let focus = self
            .focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let key = action.clone();
        row()
            .id(SharedString::from(id))
            .role(Role::Button)
            .aria_label(text.into())
            .track_focus(&focus)
            .tab_index(0)
            .flex_none()
            .h(px(24.))
            .px(px(8.))
            .justify_center()
            .rounded(px(5.))
            .text_size(px(10.))
            .line_height(px(13.5))
            .text_color(rgb(if selected { TEXT } else { MUTED }))
            .when(selected, |v| v.bg(rgb(BG)))
            .hover(|v| v.bg(style::frame_bg()))
            .focus_visible(|v| v.border_1().border_color(rgb(TEXT).opacity(0.3)))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.focus(&focus, cx);
                cx.stop_propagation();
            })
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                    this.action(key.clone(), window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_click(
                cx.listener(move |this, _, window, cx| this.action(action.clone(), window, cx)),
            )
    }
    fn text_button(
        &mut self,
        id: impl Into<String>,
        text: &str,
        selected: bool,
        action: Action,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        self.button(id, text.to_owned(), selected, action, cx)
            .child(text.to_owned())
    }
    pub(super) fn view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let wide = window.viewport_size().width >= px(1024.);
        let report = self.report.clone();
        let mut scope = row()
            .p(px(2.))
            .rounded(px(6.))
            .bg(style::frame_bg().opacity(0.55))
            .child(self.text_button(
                "usage-global",
                "All projects",
                !self.project_scope,
                Action::Scope(false),
                cx,
            ));
        let available = self.properties.project.is_some();
        scope = scope.child(
            self.text_button(
                "usage-current",
                "Current",
                self.project_scope,
                Action::Scope(true),
                cx,
            )
            .when(!available, |v| v.opacity(0.5)),
        );
        let mut periods = row()
            .p(px(2.))
            .rounded(px(6.))
            .bg(style::frame_bg().opacity(0.55));
        for days in [7, 30, 90] {
            let name = format!("{days}D");
            periods = periods.child(self.text_button(
                format!("usage-period-{days}"),
                &name,
                self.period == days,
                Action::Period(days),
                cx,
            ));
        }
        let refreshing = self.pending(Kind::Report);
        let refresh = self
            .button("usage-refresh", "Refresh usage", false, Action::Refresh, cx)
            .size(px(32.))
            .px_0()
            .when(refreshing, |v| v.opacity(0.55))
            .child(icon("refresh").size(px(14.)));
        let controls = row()
            .flex_none()
            .w_full()
            .flex_wrap()
            .justify_between()
            .gap(px(12.))
            .px(px(24.))
            .pt(px(20.))
            .child(
                label(
                    if self.project_scope {
                        self.properties
                            .project
                            .as_ref()
                            .map(|p| p.label.clone())
                            .unwrap_or_else(|| "This project".into())
                    } else {
                        "All projects".into()
                    },
                    12.,
                )
                .text_color(rgb(MUTED)),
            )
            .child(
                row()
                    .flex_none()
                    .gap(px(8.))
                    .child(scope)
                    .child(periods)
                    .child(refresh),
            );
        let mut root = column()
            .track_focus(&self.root_focus)
            .key_context("UsageDashboard")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.escape(window, cx);
                    cx.stop_propagation();
                }
            }))
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .text_size(px(14.))
            .line_height(px(18.9))
            .child(
                row()
                    .flex_none()
                    .h(px(48.))
                    .px(px(20.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(label("Usage", 14.).font_weight(FontWeight::MEDIUM)),
            )
            .child(controls);
        if self.report_error.is_some() && report.is_some() {
            root = root.child(
                label(
                    format!(
                        "Showing saved usage. Refresh failed: {}",
                        self.report_error.as_deref().unwrap()
                    ),
                    12.,
                )
                .mx(px(16.))
                .mt(px(12.))
                .px(px(12.))
                .py(px(8.))
                .rounded(px(6.))
                .bg(rgb(0x291f11))
                .text_color(rgb(0xe7c275)),
            );
        }
        if let Some(report) = report {
            let mut body = column()
                .flex_none()
                .w_full()
                .min_w_0()
                .gap(px(24.))
                .px(px(24.))
                .pr(px(32.))
                .py(px(24.));
            body = body
                .child(self.usage_chart(&report, wide, cx))
                .child(self.metrics(&report, wide))
                .child(self.limits(wide, cx))
                .child(self.contributors(&report, cx))
                .child(self.sessions(&report, cx));
            body = body.child(
                row()
                    .justify_between()
                    .text_size(px(10.))
                    .line_height(px(13.5))
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "{} sources detected",
                        report.sources.iter().filter(|s| s.detected).count()
                    ))
                    .child(if refreshing {
                        "Updating…".into()
                    } else {
                        format!(
                            "Updated {}",
                            report
                                .generated_at
                                .with_timezone(&Local)
                                .format("%-I:%M %p")
                        )
                    }),
            );
            root = root.child(self.scroll_view(body, cx));
        } else if refreshing {
            root = root.child(self.skeleton(wide));
        } else {
            root = root.child(
                column()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap(px(12.))
                    .px(px(24.))
                    .child(
                        label(
                            self.report_error
                                .clone()
                                .unwrap_or_else(|| "No usage has been recorded yet.".into()),
                            14.,
                        )
                        .text_color(rgb(MUTED)),
                    )
                    .child(
                        self.text_button(
                            "usage-retry",
                            "Refresh usage",
                            false,
                            Action::Refresh,
                            cx,
                        )
                        .h(px(28.))
                        .border_1()
                        .border_color(rgb(BORDER)),
                    ),
            );
        }
        root.into_any_element()
    }
    fn skeleton(&self, wide: bool) -> AnyElement {
        let mut blocks = row().gap(px(8.));
        for _ in 0..if wide { 4 } else { 2 } {
            blocks = blocks.child(
                div()
                    .flex_1()
                    .h(px(96.))
                    .rounded(px(6.))
                    .bg(style::frame_bg().opacity(0.4)),
            );
        }
        column()
            .flex_1()
            .min_h_0()
            .px(px(24.))
            .py(px(24.))
            .gap(px(24.))
            .child(
                div()
                    .h(px(288.))
                    .rounded(px(6.))
                    .bg(style::frame_bg().opacity(0.3)),
            )
            .child(blocks)
            .child(
                div()
                    .h(px(208.))
                    .rounded(px(6.))
                    .bg(style::frame_bg().opacity(0.2)),
            )
            .into_any_element()
    }
    fn usage_chart(&mut self, report: &Overview, wide: bool, cx: &mut Context<Self>) -> AnyElement {
        let mut title = row()
            .gap(px(8.))
            .flex_1()
            .min_w_0()
            .child(label("Daily usage", 14.).font_weight(FontWeight::MEDIUM));
        if let Some((_, _, range)) = self.range.clone() {
            title = title.child(self.text_button(
                "usage-clear-day",
                &format!("{range} · Clear"),
                false,
                Action::ClearDay,
                cx,
            ));
        }
        let choice = row()
            .p(px(2.))
            .rounded(px(6.))
            .bg(style::frame_bg().opacity(0.6))
            .child(
                self.text_button(
                    "usage-measure-tokens",
                    "Tokens",
                    !self.cost,
                    Action::Measure(false),
                    cx,
                )
                .px(px(10.)),
            )
            .child(
                self.text_button(
                    "usage-measure-cost",
                    "Cost",
                    self.cost,
                    Action::Measure(true),
                    cx,
                )
                .px(px(10.)),
            );
        let total = if self.cost {
            money(report.summary.cost_usd)
        } else {
            tokens(report.summary.tokens.total())
        };
        let hint = if self.cost {
            match report.summary.cost_status {
                CostStatus::Unknown => {
                    "Pricing is unavailable for these local usage records.".into()
                }
                CostStatus::Partial => "Some models are unpriced. Actual billing may vary.".into(),
                CostStatus::Known => {
                    "Estimated from local usage at published provider rates. Actual billing may vary."
                        .into()
                }
            }
        } else {
            format!(
                "{} locally observed usage records in this period.",
                report.summary.record_count
            )
        };
        let lead = column()
            .when(wide, |v| v.w(px(320.)).flex_none())
            .min_w_0()
            .child(
                row()
                    .gap(px(8.))
                    .child(div().size(px(6.)).rounded(px(2.)).bg(rgb(0x89ca21)))
                    .child(
                        label(
                            if self.cost {
                                "ESTIMATED API COST"
                            } else {
                                "PROCESSED TOKENS"
                            },
                            10.,
                        )
                        .text_color(rgb(MUTED)),
                    ),
            )
            .child(
                label(total, 36.)
                    .line_height(px(40.))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                label(hint, 11.)
                    .line_height(px(17.875))
                    .mt(px(4.))
                    .max_w(px(320.))
                    .text_color(rgb(MUTED)),
            );
        let mut source_rows = report.by_source.clone();
        source_rows.sort_by(|a, b| {
            let value = |r: &Group| {
                if self.cost {
                    r.summary.cost_usd.unwrap_or(0.)
                } else {
                    r.summary.tokens.total() as f64
                }
            };
            value(b).total_cmp(&value(a))
        });
        source_rows.truncate(5);
        let denom = source_rows
            .iter()
            .map(|g| {
                if self.cost {
                    g.summary.cost_usd.unwrap_or(0.)
                } else {
                    g.summary.tokens.total() as f64
                }
            })
            .sum::<f64>()
            .max(1.);
        let mut sources = div().grid().grid_cols(2).gap_x(px(24.)).gap_y(px(16.));
        for source in &source_rows {
            let n = if self.cost {
                source.summary.cost_usd.unwrap_or(0.)
            } else {
                source.summary.tokens.total() as f64
            };
            sources = sources.child(
                column()
                    .min_w_0()
                    .gap(px(6.))
                    .child(
                        row()
                            .gap(px(12.))
                            .justify_between()
                            .child(
                                label(source.label.clone(), 12.)
                                    .truncate()
                                    .text_color(rgb(MUTED)),
                            )
                            .child(
                                label(
                                    if self.cost {
                                        money(source.summary.cost_usd)
                                    } else {
                                        tokens(source.summary.tokens.total())
                                    },
                                    12.,
                                )
                                .flex_none(),
                            ),
                    )
                    .child(
                        div()
                            .h(px(4.))
                            .w_full()
                            .rounded_full()
                            .overflow_hidden()
                            .bg(style::frame_bg())
                            .child(
                                div()
                                    .h_full()
                                    .w(relative((n / denom).clamp(0., 1.) as f32))
                                    .rounded_full()
                                    .bg(rgb(TEXT).opacity(0.7)),
                            ),
                    ),
            );
        }
        let right = column()
            .flex_1()
            .min_w_0()
            .child(
                row()
                    .justify_between()
                    .mb(px(12.))
                    .text_color(rgb(MUTED))
                    .child(label(
                        if self.cost {
                            "COST BY SOURCE"
                        } else {
                            "TOKENS BY SOURCE"
                        },
                        10.,
                    ))
                    .child(label(
                        format!(
                            "{} active {}",
                            source_rows.len(),
                            if source_rows.len() == 1 {
                                "source"
                            } else {
                                "sources"
                            }
                        ),
                        10.,
                    )),
            )
            .child(sources);
        let summary = if wide {
            row()
                .items_end()
                .gap(px(24.))
                .child(lead)
                .child(right)
                .into_any_element()
        } else {
            column()
                .gap(px(24.))
                .child(lead)
                .child(right)
                .into_any_element()
        };
        let mut plot = card().p(px(16.));
        if report.groups.is_empty() {
            plot = plot.child(
                row()
                    .h(px(256.))
                    .justify_center()
                    .text_color(rgb(MUTED))
                    .child("No usage in this period."),
            );
        } else {
            let mut legend = row()
                .h(px(14.85))
                .px(px(4.))
                .gap(px(16.))
                .mb(px(12.))
                .font_family("Geist Mono");
            let entries = if self.cost {
                vec![("Estimated cost", 0x358ff3)]
            } else {
                vec![
                    ("Input", 0x358ff3),
                    ("Output", 0x28d26e),
                    ("Cache", 0x966eff),
                    ("Reasoning", 0xff9632),
                ]
            };
            for (name, color) in entries {
                legend = legend.child(
                    row()
                        .gap(px(6.))
                        .child(div().size(px(8.)).rounded(px(1.)).bg(rgb(color)))
                        .child(label(name, 11.).text_color(rgb(MUTED))),
                );
            }
            plot = plot.child(legend).child(self.chart(&report.groups, cx));
            let mut foot = row()
                .gap(px(12.))
                .mt(px(8.))
                .h(px(40.))
                .px(px(4.))
                .border_t_1()
                .border_color(rgb(BORDER))
                .text_size(px(11.))
                .line_height(px(14.85));
            if let Some((index, group)) = self
                .hovered
                .and_then(|i| report.groups.get(i).map(|g| (i, g)))
            {
                foot = foot
                    .child(label(group.label.clone(), 11.))
                    .child(
                        label(
                            if self.cost {
                                money(group.summary.cost_usd)
                            } else {
                                tokens(group.summary.tokens.total())
                            },
                            11.,
                        )
                        .font_weight(FontWeight::MEDIUM),
                    )
                    .child(
                        self.text_button(
                            "usage-view-day",
                            "View day",
                            false,
                            Action::Day(index),
                            cx,
                        )
                        .ml_auto(),
                    );
            } else {
                foot = foot.child(
                    label("Hover the chart for a daily breakdown", 11.).text_color(rgb(MUTED)),
                );
            }
            plot = plot.child(foot);
        }
        frame()
            .child(
                row()
                    .h(px(44.))
                    .px(px(12.))
                    .gap(px(12.))
                    .child(title)
                    .child(choice),
            )
            .child(
                column()
                    .gap(px(4.))
                    .child(card().p(px(20.)).child(summary))
                    .child(plot),
            )
            .into_any_element()
    }
    fn metrics(&self, report: &Overview, wide: bool) -> AnyElement {
        let summary = &report.summary;
        let previous = report.previous_summary.as_ref();
        let rows = [
            (
                "Cost",
                money(summary.cost_usd),
                match summary.cost_status {
                    CostStatus::Partial => "Some models are unpriced",
                    CostStatus::Unknown => "Pricing unavailable",
                    CostStatus::Known => "Estimated from local usage",
                }
                .into(),
                delta(summary.cost_usd, previous.and_then(|s| s.cost_usd)),
            ),
            (
                "Tokens",
                tokens(summary.tokens.total()),
                format!(
                    "{} in · {} out",
                    tokens(summary.tokens.input_tokens),
                    tokens(summary.tokens.output_tokens)
                ),
                delta(
                    Some(summary.tokens.total() as f64),
                    previous.map(|s| s.tokens.total() as f64),
                ),
            ),
            (
                "Cache reads",
                tokens(summary.tokens.cache_read_tokens),
                format!(
                    "{}% of observed input",
                    ((summary.tokens.cache_read_tokens as f64
                        / (summary
                            .tokens
                            .input_tokens
                            .saturating_add(summary.tokens.cache_read_tokens)
                            .max(1) as f64))
                        * 100.)
                        .round() as u64
                ),
                delta(
                    Some(summary.tokens.cache_read_tokens as f64),
                    previous.map(|s| s.tokens.cache_read_tokens as f64),
                ),
            ),
            (
                "Active sessions",
                report.session_count.to_string(),
                format!("{} usage records", summary.record_count),
                delta(
                    Some(report.session_count as f64),
                    report.previous_session_count.map(|s| s as f64),
                ),
            ),
        ];
        let mut content = div().grid().grid_cols(if wide { 4 } else { 2 }).gap(px(4.));
        for (name, value, hint, change) in rows {
            content = content.child(
                card()
                    .min_h(px(96.))
                    .p(px(16.))
                    .child(
                        row()
                            .gap(px(8.))
                            .justify_between()
                            .text_color(rgb(MUTED))
                            .child(label(name, 11.).font_weight(FontWeight::MEDIUM))
                            .when_some(change, |v, d| v.child(label(d, 10.))),
                    )
                    .child(
                        label(value, 20.)
                            .line_height(px(28.))
                            .font_weight(FontWeight::MEDIUM)
                            .mt(px(8.))
                            .truncate(),
                    )
                    .child(
                        label(hint, 10.)
                            .text_color(rgb(MUTED))
                            .mt(px(8.))
                            .truncate(),
                    ),
            );
        }
        frame().child(content).into_any_element()
    }
    fn limits(&self, wide: bool, _cx: &mut Context<Self>) -> AnyElement {
        let mut cards = div().grid().grid_cols(if wide { 2 } else { 1 }).gap(px(4.));
        for (harness, _, name, art) in PROVIDERS {
            let account = self
                .accounts
                .accounts
                .iter()
                .find(|a| a.harness == harness && a.active);
            let primary = account.and_then(|a| a.usage_windows.first());
            let mut info = column().flex_1().min_w_0();
            let mut heading = row().justify_between().gap(px(12.)).child(
                column()
                    .min_w_0()
                    .child(label(name, 12.).font_weight(FontWeight::MEDIUM))
                    .child(
                        label(
                            account
                                .and_then(|a| a.plan_label.clone())
                                .or_else(|| primary.map(|w| w.label.clone()))
                                .unwrap_or_else(|| "Usage limits".into()),
                            10.,
                        )
                        .truncate()
                        .text_color(rgb(MUTED)),
                    ),
            );
            let right = column()
                .items_end()
                .flex_none()
                .child(
                    label(
                        primary
                            .map(|w| {
                                format!("{:.0}% left", (1. - w.used_fraction.clamp(0., 1.)) * 100.)
                            })
                            .unwrap_or_else(|| "—".into()),
                        14.,
                    )
                    .line_height(px(20.))
                    .font_weight(FontWeight::MEDIUM),
                )
                .child(
                    label(
                        primary
                            .and_then(|w| w.resets_at)
                            .map(|at| {
                                at.with_timezone(&Local)
                                    .format("%b %-d, %-I:%M %p")
                                    .to_string()
                            })
                            .unwrap_or_else(|| {
                                if primary.is_some() {
                                    "No reset".into()
                                } else {
                                    "Usage unavailable".into()
                                }
                            }),
                        10.,
                    )
                    .text_color(rgb(MUTED)),
                );
            heading = heading.child(right);
            info = info.child(heading);
            if let Some(primary) = primary {
                info = info.child(meter(primary.used_fraction).mt(px(8.)));
            } else if let Some(error) = account
                .and_then(|a| a.usage_error.as_deref())
                .or(self.limits_error.as_deref())
            {
                info = info.child(
                    label(error.to_owned(), 10.)
                        .text_color(rgb(MUTED))
                        .mt(px(4.)),
                );
            }
            cards = cards.child(
                card().min_h(px(80.)).p(px(12.)).child(
                    row()
                        .gap(px(12.))
                        .child(
                            row()
                                .size(px(32.))
                                .flex_none()
                                .justify_center()
                                .rounded(px(6.))
                                .bg(style::frame_bg().opacity(0.45))
                                .child(icon(art).size(px(16.))),
                        )
                        .child(info),
                ),
            );
        }
        frame()
            .child(
                row()
                    .px(px(12.))
                    .py(px(8.))
                    .justify_between()
                    .child(label("Plan limits", 14.).font_weight(FontWeight::MEDIUM))
                    .child(label("Live provider allowance", 11.).text_color(rgb(MUTED))),
            )
            .child(cards)
            .into_any_element()
    }
    fn contributors(&mut self, report: &Overview, cx: &mut Context<Self>) -> AnyElement {
        let mut tabs = row()
            .p(px(2.))
            .rounded(px(6.))
            .bg(style::frame_bg().opacity(0.6));
        for (i, name) in ["Providers", "Models", "Projects"].iter().enumerate() {
            tabs = tabs.child(self.text_button(
                format!("usage-contributor-{i}"),
                name,
                self.contributor == i,
                Action::Contributors(i),
                cx,
            ));
        }
        let (mut rows, previous, title) = match self.contributor {
            1 => (report.by_model.clone(), &report.previous_by_model, "Model"),
            2 => (
                report.by_project.clone(),
                &report.previous_by_project,
                "Project",
            ),
            _ => (
                report.by_source.clone(),
                &report.previous_by_source,
                "Provider",
            ),
        };
        rows.sort_by_key(|r| std::cmp::Reverse(r.summary.tokens.total()));
        let total = rows
            .iter()
            .map(|r| r.summary.tokens.total() as f64)
            .sum::<f64>()
            .max(1.);
        let count = rows.len();
        if !self.expanded {
            rows.truncate(5);
        }
        let mut table = column().child(table_row(
            vec![
                (title.into(), 0.30),
                ("Share".into(), 0.30),
                ("Tokens".into(), 0.20),
                ("Cost".into(), 0.20),
            ],
            true,
        ));
        if rows.is_empty() {
            table = table.child(
                row()
                    .h(px(64.))
                    .justify_center()
                    .child(label("No contributors found.", 12.).text_color(rgb(MUTED))),
            );
        }
        for group in rows {
            let previous = previous.iter().find(|p| p.key == group.key);
            let change = delta(
                Some(group.summary.tokens.total() as f64),
                previous.map(|p| p.summary.tokens.total() as f64),
            );
            let share = format!(
                "{:.1}%{}",
                group.summary.tokens.total() as f64 / total * 100.,
                change.map(|d| format!(" · {d}")).unwrap_or_default()
            );
            table = table.child(table_row(
                vec![
                    (group.label, 0.30),
                    (share, 0.30),
                    (tokens(group.summary.tokens.total()), 0.20),
                    (money(group.summary.cost_usd), 0.20),
                ],
                false,
            ));
        }
        let mut result = frame()
            .child(
                row()
                    .h(px(44.))
                    .px(px(12.))
                    .justify_between()
                    .child(label("Breakdown", 14.).font_weight(FontWeight::MEDIUM))
                    .child(tabs),
            )
            .child(card().overflow_hidden().child(table));
        if count > 5 {
            let expanded = self.expanded;
            result = result.child(row().p(px(8.)).justify_center().child(self.text_button(
                "usage-expand-contributors",
                &if expanded {
                    "Show less".into()
                } else {
                    format!("View all {count}")
                },
                false,
                Action::Expand,
                cx,
            )));
        }
        result.into_any_element()
    }
    fn sessions(&mut self, report: &Overview, cx: &mut Context<Self>) -> AnyElement {
        let heading = self
            .button(
                "usage-sessions-toggle",
                "Sessions",
                false,
                Action::ToggleSessions,
                cx,
            )
            .h_auto()
            .w_full()
            .px(px(16.))
            .py(px(12.))
            .justify_between()
            .child(
                column()
                    .items_start()
                    .child(
                        label("Sessions", 14.)
                            .line_height(px(20.))
                            .font_weight(FontWeight::MEDIUM),
                    )
                    .child(
                        label(format!("{} in this period", report.session_count), 10.)
                            .text_color(rgb(MUTED)),
                    ),
            )
            .child(
                icon(if self.sessions_open {
                    "alt-arrow-up"
                } else {
                    "alt-arrow-down"
                })
                .size(px(16.)),
            );
        let mut result = frame().child(heading);
        if self.sessions_open {
            let name = self
                .provider
                .and_then(|id| PROVIDERS.iter().find(|p| p.1 == id).map(|p| p.2))
                .unwrap_or("All providers");
            let mut provider = self
                .text_button("usage-provider", name, false, Action::ProviderMenu, cx)
                .relative()
                .h(px(28.))
                .px(px(8.))
                .text_size(px(12.))
                .bg(style::card_bg())
                .border_1()
                .border_color(rgb(BORDER))
                .child(icon("alt-arrow-down").size(px(12.)).ml(px(6.)));
            if self.provider_open {
                let mut menu = column()
                    .w(px(160.))
                    .p(px(4.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(style::card_bg())
                    .shadow_lg()
                    .child(
                        self.text_button(
                            "usage-provider-all",
                            "All providers",
                            self.provider.is_none(),
                            Action::Provider(None),
                            cx,
                        )
                        .h(px(28.))
                        .justify_start(),
                    );
                for (_, id, label, _) in PROVIDERS {
                    menu = menu.child(
                        self.text_button(
                            format!("usage-filter-{id}"),
                            label,
                            self.provider == Some(id),
                            Action::Provider(Some(id)),
                            cx,
                        )
                        .h(px(28.))
                        .justify_start(),
                    );
                }
                provider = provider.child(deferred(
                    div().absolute().top(px(32.)).right_0().child(menu),
                ));
            }
            let search = row()
                .flex_1()
                .min_w_0()
                .h(px(28.))
                .px(px(10.))
                .gap(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(BORDER))
                .bg(style::card_bg())
                .child(icon("magnifer").size(px(14.)).text_color(rgb(MUTED)))
                .child(self.query.clone());
            result = result.child(
                row()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(search)
                    .child(provider),
            );
            if let Some(error) = self.page_error.clone() {
                result = result.child(
                    row()
                        .px(px(12.))
                        .py(px(8.))
                        .gap(px(8.))
                        .child(label(error, 12.).flex_1().text_color(rgb(0xe5a0a0)))
                        .child(self.text_button(
                            "usage-sessions-retry",
                            "Retry",
                            false,
                            Action::RetrySessions,
                            cx,
                        )),
                );
            }
            let mut headers = row()
                .h(px(28.))
                .border_b_1()
                .border_color(rgb(BORDER))
                .child(
                    label("Session", 11.)
                        .text_color(rgb(MUTED))
                        .w(relative(0.46))
                        .px(px(12.)),
                );
            for (id, name, sort) in [
                ("active", "Last active", SessionSort::LastActive),
                ("tokens", "Tokens", SessionSort::Tokens),
                ("cost", "Cost", SessionSort::Cost),
            ] {
                let active = std::mem::discriminant(&self.sort) == std::mem::discriminant(&sort);
                headers = headers.child(
                    self.text_button(
                        format!("usage-sort-{id}"),
                        name,
                        false,
                        Action::Sort(sort),
                        cx,
                    )
                    .flex_1()
                    .min_w_0()
                    .justify_end()
                    .h(px(28.))
                    .child(icon("alt-arrow-down").size(px(12.)).ml(px(4.)))
                    .when(!active, |v| v.opacity(0.6)),
                );
            }
            let mut table = column().child(headers);
            let loading = self.pending(Kind::Sessions);
            if let Some(page) = self.page.as_ref() {
                for group in &page.rows {
                    table = table.child(
                        table_row(
                            vec![
                                (group.label.clone(), 0.46),
                                (date(group), 0.18),
                                (tokens(group.summary.tokens.total()), 0.18),
                                (money(group.summary.cost_usd), 0.18),
                            ],
                            false,
                        )
                        .h(px(36.)),
                    );
                }
                if page.rows.is_empty() {
                    table = table.child(row().h(px(80.)).justify_center().child(
                        label("No sessions match these filters.", 12.).text_color(rgb(MUTED)),
                    ));
                }
            } else {
                table = table.child(
                    row().h(px(80.)).justify_center().child(
                        label(
                            if loading {
                                "Loading sessions…"
                            } else {
                                "No sessions to display."
                            },
                            12.,
                        )
                        .text_color(rgb(MUTED)),
                    ),
                );
            }
            result = result.child(card().rounded(px(0.)).child(table));
            let total = self
                .page
                .as_ref()
                .map(|p| p.total)
                .unwrap_or(report.session_count);
            let page = self.offset / 10;
            let count = total.div_ceil(10).max(1);
            let back = self
                .text_button(
                    "usage-page-back",
                    "‹",
                    false,
                    Action::Page(self.offset.saturating_sub(10)),
                    cx,
                )
                .when(page == 0, |v| v.opacity(0.3));
            let next_offset = self.page.as_ref().and_then(|p| p.next_offset);
            let next = self
                .text_button(
                    "usage-page-next",
                    "›",
                    false,
                    Action::Page(next_offset.unwrap_or(self.offset)),
                    cx,
                )
                .when(next_offset.is_none(), |v| v.opacity(0.3));
            result = result.child(
                row()
                    .justify_between()
                    .px(px(12.))
                    .py(px(8.))
                    .child(
                        label(
                            format!("Page {} of {} · {} sessions", page + 1, count, total),
                            11.,
                        )
                        .text_color(rgb(MUTED)),
                    )
                    .child(row().gap(px(4.)).child(back).child(next)),
            );
        }
        result.into_any_element()
    }
}
fn table_row(values: Vec<(String, f32)>, header: bool) -> Div {
    let mut line = row()
        .h(px(28.))
        .w_full()
        .border_b_1()
        .border_color(rgb(BORDER).opacity(0.6));
    for (i, (text, width)) in values.into_iter().enumerate() {
        line = line.child(
            label(text, if header { 11. } else { 12. })
                .w(relative(width))
                .min_w_0()
                .px(px(12.))
                .truncate()
                .when(i >= 2, |v| v.text_align(TextAlign::Right))
                .when(header || i == 1 || i == 3, |v| v.text_color(rgb(MUTED))),
        );
    }
    line
}
fn meter(fraction: f32) -> Div {
    let mut bar = row().w_full().h(px(16.)).gap(px(2.));
    let filled = (fraction.clamp(0., 1.) * 38.).ceil();
    for i in 0..38 {
        bar = bar.child(
            div()
                .flex_1()
                .h_full()
                .rounded(px(2.))
                .bg(if (i as f32) < filled {
                    rgb(if fraction >= 0.8 { 0xfcd34d } else { 0x89ca21 })
                        .opacity(if fraction >= 0.8 { 0.65 } else { 0.7 })
                } else {
                    rgb(MUTED).opacity(0.16)
                }),
        );
    }
    bar
}
