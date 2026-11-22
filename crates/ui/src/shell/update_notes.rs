//! The hover card on the sidebar update pill: the update's status plus what
//! the offered release changes.

use crate::theme::Theme;
use gpui::{
    Context, FontWeight, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::*, px,
};
use zuse_update::ReleaseNotesSection;

/// Enough to judge the release at a glance; the link has the rest.
const MAX_ITEMS: usize = 6;

pub(super) struct UpdateNotesCard {
    pub status: SharedString,
    pub version: String,
    pub sections: Vec<ReleaseNotesSection>,
}

impl Render for UpdateNotesCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let url = zuse_update::release_page(&self.version);
        let total: usize = self.sections.iter().map(|s| s.items.len()).sum();
        let mut remaining = MAX_ITEMS;
        let mut changes = div().flex().flex_col().gap(px(8.0));
        for section in &self.sections {
            if remaining == 0 {
                break;
            }
            let shown = section.items.len().min(remaining);
            remaining -= shown;
            changes = changes.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_muted)
                            .child(section.title.clone()),
                    )
                    .children(section.items[..shown].iter().map(|item| {
                        div()
                            .line_clamp(2)
                            .text_color(theme.text)
                            .child(item.clone())
                    })),
            );
        }
        let hidden = total.saturating_sub(MAX_ITEMS);
        let card = div()
            .occlude()
            .w(px(320.0))
            .p(px(10.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .rounded(px(7.0))
            .border_1()
            .border_color(theme.border)
            .bg(crate::popover::surface_bg(theme))
            .text_size(px(11.0))
            .text_color(theme.text_muted)
            .child(self.status.clone())
            .child(
                div()
                    .text_size(crate::typography::text_xs())
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(format!("What's new in v{}", self.version)),
            )
            .child(changes)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(if hidden > 0 {
                        format!("+{hidden} more")
                    } else {
                        String::new()
                    })
                    .child(
                        div()
                            .id("update-notes-release-page")
                            .cursor_pointer()
                            .text_color(theme.accent)
                            .hover(|el| el.underline())
                            .child("Full release notes")
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    ),
            );
        crate::frost::frosted(6.0, crate::frost::MENU_BLUR, card)
    }
}
