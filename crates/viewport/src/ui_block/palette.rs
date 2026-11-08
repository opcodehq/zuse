//! Colour and font tokens a UI block paints with. Hosts map their theme onto
//! these roles; [`UiBlockPalette::reference`] holds the reference tokens.

use gpui::{Hsla, SharedString, hsla};

/// Theme roles used by UI block components.
#[derive(Clone, Debug, PartialEq)]
pub struct UiBlockPalette {
    pub foreground: Hsla,
    pub muted_foreground: Hsla,
    /// Tonal surface; components use it at low alpha (e.g. 40%).
    pub muted: Hsla,
    pub border: Hsla,
    /// Form control borders and slider track.
    pub input: Hsla,
    /// Form control and outline button fill.
    pub card: Hsla,
    /// Tooltip and select menu fill.
    pub popover: Hsla,
    /// Page background: the selected tab fill.
    pub background: Hsla,
    pub primary: Hsla,
    pub primary_foreground: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub destructive: Hsla,
    pub alert_success_bg: Hsla,
    pub alert_warning_bg: Hsla,
    pub alert_error_bg: Hsla,
    pub dark: bool,
    pub font_sans: SharedString,
    pub font_mono: SharedString,
}

fn hsl(h: f32, s: f32, l: f32) -> Hsla {
    hsla(h / 360.0, s / 100.0, l / 100.0, 1.0)
}

impl UiBlockPalette {
    /// The reference light or dark tokens with the default interface fonts.
    pub fn reference(dark: bool) -> Self {
        let white = hsla(0.0, 0.0, 1.0, 1.0);
        if dark {
            Self {
                background: hsl(140.0, 5.0, 4.5),
                foreground: hsl(100.0, 4.0, 91.0),
                muted: hsl(132.0, 5.0, 14.0),
                muted_foreground: hsl(105.0, 3.0, 65.0),
                border: hsl(132.0, 4.0, 12.5),
                input: hsl(128.0, 5.0, 17.0),
                card: hsl(136.0, 5.0, 7.5),
                popover: hsl(132.0, 5.0, 10.5),
                primary: hsl(83.0, 72.0, 46.0),
                primary_foreground: hsl(132.0, 30.0, 7.0),
                success: hsl(133.9, 60.6, 48.8),
                warning: hsl(21.6, 98.2, 56.7),
                destructive: hsl(0.0, 92.1, 60.4),
                alert_success_bg: hsl(132.9, 35.9, 15.3),
                alert_warning_bg: hsl(20.8, 28.9, 17.6),
                alert_error_bg: hsl(349.3, 31.1, 17.6),
                dark,
                font_sans: "Geist".into(),
                font_mono: "Geist Mono".into(),
            }
        } else {
            Self {
                background: hsl(90.0, 20.0, 97.0),
                foreground: hsl(132.0, 14.0, 13.0),
                muted: hsl(105.0, 18.0, 94.0),
                muted_foreground: hsl(125.0, 7.0, 38.0),
                border: hsl(110.0, 13.0, 89.0),
                input: hsl(112.0, 12.0, 82.0),
                card: white,
                popover: white,
                primary: hsl(83.0, 74.0, 43.0),
                primary_foreground: hsl(132.0, 30.0, 7.0),
                success: hsl(131.4, 73.4, 31.0),
                warning: hsl(21.6, 98.2, 56.7),
                destructive: hsl(0.0, 74.2, 52.9),
                alert_success_bg: hsl(130.0, 50.0, 95.3),
                alert_warning_bg: hsl(22.1, 100.0, 96.3),
                alert_error_bg: hsl(0.0, 100.0, 97.3),
                dark,
                font_sans: "Geist".into(),
                font_mono: "Geist Mono".into(),
            }
        }
    }

    /// The 2% black (light) or white (dark) wash for hovered table rows.
    pub(super) fn row_hover(&self) -> Hsla {
        if self.dark {
            hsla(0.0, 0.0, 1.0, 0.02)
        } else {
            hsla(0.0, 0.0, 0.0, 0.02)
        }
    }
}
