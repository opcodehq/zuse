//! Authored Zuse app-palette colors for the current GPUI render pass.
use std::cell::Cell;
thread_local! { static LIGHT: Cell<bool> = const { Cell::new(false) }; }
pub fn set_light(light: bool) {
    LIGHT.set(light);
}
pub fn is_light() -> bool {
    LIGHT.get()
}
pub fn rgb(value: u32) -> gpui::Rgba {
    let value = if is_light() {
        match value {
            0x0b0c0b => 0xf7f9f6,
            0x0e0f0e => 0xf1f5ef,
            0x121413 | 0x101010 | 0x141414 | 0x181916 => 0xffffff,
            0x191c1a | 0x171717 | 0x181818 | 0x111111 | 0x13150f | 0x10110e | 0x191a17
            | 0x1a1a1a | 0x191919 | 0x1d1d1d | 0x1c1d19 | 0x20211e => 0xe3e9e2,
            0x202020 | 0x222222 | 0x232323 | 0x24251f | 0x292929 | 0x2a2a2a | 0x2b2b28 => 0xe0e8de,
            0x343d22 | 0x2b2d26 | 0x2c351d | 0x34372c | 0x32372f => 0xe2edce,
            0x33352d | 0x343434 | 0x383838 | 0x383a34 | 0x3b3d34 | 0x444444 => 0xcdd7cc,
            0x1f211f | 0x242424 | 0x303030 | 0x30312b => 0xe1e7df,
            0xe8e9e7 | 0xebebeb | 0xd0d0d0 | 0xe5e5e5 => 0x1d261e,
            0xa4a8a3 | 0xa3a3a3 | 0x929292 => 0x5a685b,
            0x6b716a | 0x777777 => 0x748175,
            0x89ca21 => 0x81bf1d,
            value => value,
        }
    } else {
        value
    };
    gpui::rgb(value)
}
pub fn tone(dark: gpui::Hsla, light: gpui::Hsla) -> gpui::Hsla {
    if is_light() { light } else { dark }
}
