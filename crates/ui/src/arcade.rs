//! Arcade accents: small self-playing pixel scenes that give quiet surfaces
//! some life without competing with the content around them.
//!
//! - [`invaders_header`]: a dim band for the top of the new-session screen.
//!   Rows of invaders march above a sliding ship and dissolve toward the
//!   bottom, so it reads as texture, not a game.
//! - [`pellet_trail`]: a one-line strip where a little pixel bug walks along a
//!   row of pellets, eating them, while an agent works.
//!
//! Both are grayscale tints of the caller's color, repaint from their own
//! cached view on the shared pulse clock, and freeze on one frame under
//! reduced motion. Scene math lives in pure functions so it is unit-tested.

use gpui::{
    App, AppContext, Context, Entity, Hsla, IntoElement, ParentElement, Render, RenderOnce,
    SharedString, Styled, Window, canvas, div, point, px, size,
};

use crate::motion::{self, MotionSpec};

/// Long wall-clock period for the arcade clock; scenes derive their own
/// cadence from elapsed seconds, so the wrap is effectively never seen.
const ARCADE_CLOCK: MotionSpec = MotionSpec::new(3_600_000, motion::EASE);

fn elapsed_secs(view: gpui::EntityId, cx: &mut App) -> f32 {
    motion::pulse_delta_slow(&ARCADE_CLOCK, view, cx) * ARCADE_CLOCK.total().as_secs_f32()
}

fn bayer(x: usize, y: usize) -> f32 {
    zuse_viewport::dither::threshold(x, y)
}

fn fill(
    origin: gpui::Point<gpui::Pixels>,
    x: f32,
    y: f32,
    side: f32,
    color: Hsla,
    window: &mut Window,
) {
    window.paint_quad(gpui::fill(
        gpui::Bounds::new(origin + point(px(x), px(y)), size(px(side), px(side))),
        color,
    ));
}

// ---------------------------------------------------------------------------
// Invaders header
// ---------------------------------------------------------------------------

const INVADER_PIXEL: f32 = 2.0;
const INVADER_PITCH: f32 = 46.0;
const INVADER_ROWS: usize = 3;
const INVADER_TOP: f32 = 20.0;
const INVADER_ROW_GAP: f32 = 34.0;
const INVADER_MARGIN: f32 = 30.0;
/// The formation drifts `±INVADER_SWAY`px at this many px per second.
const INVADER_SWAY: f32 = 15.0;
const INVADER_SWAY_SPEED: f32 = 9.0;
/// Fraction of the formation already shot down.
const INVADER_GAPS: u32 = 17;
const SHIP_OFFSET: f32 = 34.0;
const SHOT_SPEED: f32 = 150.0;
const SHOT_EVERY: f32 = 2.2;

const INVADER: [[&str; 8]; 2] = [
    [
        "..#.....#..",
        "...#...#...",
        "..#######..",
        ".##.###.##.",
        "###########",
        "#.#######.#",
        "#.#.....#.#",
        "...##.##...",
    ],
    [
        "..#.....#..",
        "#..#...#..#",
        "#.#######.#",
        "###.###.###",
        "###########",
        ".#########.",
        "..#.....#..",
        ".#.......#.",
    ],
];
const SHIP: [&str; 4] = [".....#.....", "....###....", ".#########.", "###########"];
const INVADER_W: f32 = 11.0 * INVADER_PIXEL;

fn invader_columns(width: f32) -> usize {
    ((width - INVADER_MARGIN * 2.0) / INVADER_PITCH).max(0.0) as usize
}

/// A stable pattern of survivors, so the formation looks mid-game.
fn invader_alive(column: usize, row: usize) -> bool {
    let hash = (column as u32)
        .wrapping_mul(2_654_435_761)
        .wrapping_add((row as u32 + 1).wrapping_mul(40_503));
    (hash ^ (hash >> 13)) % 100 >= INVADER_GAPS
}

/// How much of a row at `y` survives the dissolve: solid at the top of the
/// band, gone at the bottom.
fn invader_density(y: f32, height: f32) -> f32 {
    ((1.0 - y / height.max(1.0)) / 0.7).clamp(0.0, 1.0)
}

/// Sway offset: a triangle wave, so the formation turns at the edges.
fn invader_sway(elapsed: f32) -> f32 {
    let travel = INVADER_SWAY * 2.0;
    let phase = (elapsed * INVADER_SWAY_SPEED).rem_euclid(travel * 2.0);
    (if phase < travel {
        phase
    } else {
        travel * 2.0 - phase
    }) - INVADER_SWAY
}

fn ship_x(width: f32, elapsed: f32) -> f32 {
    width / 2.0 + (elapsed * 0.6).sin() * width * 0.36
}

/// The current shot's position, while one is in flight: fired from the ship
/// and spent on reaching the formation.
fn shot_position(width: f32, height: f32, elapsed: f32) -> Option<(f32, f32)> {
    let age = elapsed.rem_euclid(SHOT_EVERY);
    let muzzle = height - SHIP_OFFSET;
    let y = muzzle - age * SHOT_SPEED;
    let formation_bottom = INVADER_TOP + INVADER_ROW_GAP * INVADER_ROWS as f32;
    (y > formation_bottom).then(|| (ship_x(width, elapsed - age) + INVADER_W / 2.0 - 1.0, y))
}

fn paint_sprite(
    origin: gpui::Point<gpui::Pixels>,
    sprite: &[&str],
    (x, y): (f32, f32),
    color: Hsla,
    window: &mut Window,
) {
    for (row, line) in sprite.iter().enumerate() {
        for (col, cell) in line.bytes().enumerate() {
            if cell == b'#' {
                fill(
                    origin,
                    x + col as f32 * INVADER_PIXEL,
                    y + row as f32 * INVADER_PIXEL,
                    INVADER_PIXEL,
                    color,
                    window,
                );
            }
        }
    }
}

/// Rows of invaders marching above a sliding ship. Fill a positioned band
/// with it; `tint` is usually the theme's text color.
pub fn invaders_header(key: impl Into<SharedString>, height: f32, tint: Hsla) -> impl IntoElement {
    InvadersHeader {
        key: key.into(),
        height,
        tint,
    }
}

#[derive(IntoElement)]
struct InvadersHeader {
    key: SharedString,
    height: f32,
    tint: Hsla,
}

impl RenderOnce for InvadersHeader {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let height = self.height;
        let view = window.with_global_id(self.key.into(), |id, window| {
            window.with_element_state(id, |previous: Option<Entity<InvadersView>>, _| {
                let view = previous.unwrap_or_else(|| cx.new(|_| InvadersView { tint: self.tint }));
                view.update(cx, |view, cx| {
                    if view.tint != self.tint {
                        view.tint = self.tint;
                        cx.notify();
                    }
                });
                (view.clone(), view)
            })
        });
        view.cached(gpui::StyleRefinement::default().w_full().h(px(height)))
    }
}

struct InvadersView {
    tint: Hsla,
}

impl Render for InvadersView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let elapsed = elapsed_secs(cx.entity_id(), cx);
        let tint = self.tint;
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let width = f32::from(bounds.size.width);
                let height = f32::from(bounds.size.height);
                let sway = invader_sway(elapsed);
                let sprite = &INVADER[(elapsed * 1.6) as usize % 2];
                for row in 0..INVADER_ROWS {
                    let y = INVADER_TOP + row as f32 * INVADER_ROW_GAP;
                    let density = invader_density(y, height);
                    let alpha = 0.2 - row as f32 * 0.04;
                    for column in 0..invader_columns(width) {
                        if !invader_alive(column, row) {
                            continue;
                        }
                        let x = INVADER_MARGIN + column as f32 * INVADER_PITCH + sway;
                        for (j, line) in sprite.iter().enumerate() {
                            for (i, cell) in line.bytes().enumerate() {
                                if cell == b'#' && density > bayer(i, j) {
                                    fill(
                                        bounds.origin,
                                        x + i as f32 * INVADER_PIXEL,
                                        y + j as f32 * INVADER_PIXEL,
                                        INVADER_PIXEL,
                                        tint.opacity(alpha),
                                        window,
                                    );
                                }
                            }
                        }
                    }
                }
                paint_sprite(
                    bounds.origin,
                    &SHIP,
                    (ship_x(width, elapsed), height - SHIP_OFFSET),
                    tint.opacity(0.16),
                    window,
                );
                if let Some((x, y)) = shot_position(width, height, elapsed) {
                    window.paint_quad(gpui::fill(
                        gpui::Bounds::new(
                            bounds.origin + point(px(x), px(y)),
                            size(px(2.0), px(8.0)),
                        ),
                        tint.opacity(0.3),
                    ));
                }
            },
        )
        .size_full()
    }
}

// ---------------------------------------------------------------------------
// Pellet trail
// ---------------------------------------------------------------------------

/// Sprite pixel side and pellet spacing, in logical pixels.
const BUG_PIXEL: f32 = 1.5;
const PELLET_GAP: f32 = 12.0;
const PELLET_SIDE: f32 = 2.0;
/// Walk speed in logical pixels per second.
const BUG_SPEED: f32 = 70.0;

/// The bug, 7×5, in two walk frames (legs alternate).
const BUG: [[&str; 5]; 2] = [
    [".#...#.", "..###..", ".#####.", "##.#.##", "#.#.#.#"],
    [".#...#.", "..###..", ".#####.", "##.#.##", ".#.#.#."],
];
const BUG_W: f32 = 7.0 * BUG_PIXEL;
const BUG_H: f32 = 5.0 * BUG_PIXEL;
/// The trail's painted height, for callers positioning it.
pub const PELLET_TRAIL_HEIGHT: f32 = BUG_H;

struct TrailFrame {
    /// The bug's left edge.
    x: f32,
    walk: usize,
    /// Pellets ahead of the bug's mouth; those behind it are eaten.
    first_pellet: usize,
    pellets: usize,
}

fn trail_frame(width: f32, elapsed: f32) -> TrailFrame {
    let pellets = (width / PELLET_GAP).floor().max(0.0) as usize;
    let span = width + BUG_W * 2.0;
    let x = (elapsed * BUG_SPEED).rem_euclid(span) - BUG_W;
    let mouth = x + BUG_W;
    let first_pellet = ((mouth - PELLET_GAP / 2.0) / PELLET_GAP).ceil().max(0.0) as usize;
    TrailFrame {
        x,
        walk: ((elapsed * BUG_SPEED) / (BUG_PIXEL * 4.0)) as usize % 2,
        first_pellet: first_pellet.min(pellets),
        pellets,
    }
}

/// A pixel bug walking a row of pellets and eating them, for "working"
/// states. Fills its parent's width, [`PELLET_TRAIL_HEIGHT`] tall.
pub fn pellet_trail(key: impl Into<SharedString>, tint: Hsla) -> impl IntoElement {
    PelletTrail {
        key: key.into(),
        tint,
    }
}

#[derive(IntoElement)]
struct PelletTrail {
    key: SharedString,
    tint: Hsla,
}

impl RenderOnce for PelletTrail {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let view = window.with_global_id(self.key.into(), |id, window| {
            window.with_element_state(id, |previous: Option<Entity<TrailView>>, _| {
                let view = previous.unwrap_or_else(|| cx.new(|_| TrailView { tint: self.tint }));
                view.update(cx, |view, cx| {
                    if view.tint != self.tint {
                        view.tint = self.tint;
                        cx.notify();
                    }
                });
                (view.clone(), view)
            })
        });
        div()
            .w_full()
            .h(px(BUG_H))
            .child(view.cached(gpui::StyleRefinement::default().w_full().h(px(BUG_H))))
    }
}

struct TrailView {
    tint: Hsla,
}

impl Render for TrailView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let elapsed = elapsed_secs(cx.entity_id(), cx);
        let tint = self.tint;
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let width = f32::from(bounds.size.width);
                let frame = trail_frame(width, elapsed);
                let pellet_y = (BUG_H - PELLET_SIDE) / 2.0;
                for i in frame.first_pellet..frame.pellets {
                    let x = i as f32 * PELLET_GAP + (PELLET_GAP - PELLET_SIDE) / 2.0;
                    fill(
                        bounds.origin,
                        x,
                        pellet_y,
                        PELLET_SIDE,
                        tint.opacity(0.28),
                        window,
                    );
                }
                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    for (row, line) in BUG[frame.walk].iter().enumerate() {
                        for (col, cell) in line.bytes().enumerate() {
                            if cell == b'#' {
                                fill(
                                    bounds.origin,
                                    frame.x + col as f32 * BUG_PIXEL,
                                    row as f32 * BUG_PIXEL,
                                    BUG_PIXEL,
                                    tint.opacity(0.7),
                                    window,
                                );
                            }
                        }
                    }
                });
            },
        )
        .size_full()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invaders_sway_stays_inside_its_travel() {
        for step in 0..400 {
            let sway = invader_sway(step as f32 * 0.1);
            assert!((-INVADER_SWAY..=INVADER_SWAY).contains(&sway));
        }
    }

    #[test]
    fn formation_has_survivors_and_gaps() {
        let alive = (0..60)
            .flat_map(|column| (0..INVADER_ROWS).map(move |row| (column, row)))
            .filter(|&(column, row)| invader_alive(column, row))
            .count();
        assert!(alive > 90 && alive < 180, "{alive}");
    }

    #[test]
    fn formation_dissolves_toward_the_bottom() {
        assert_eq!(invader_density(0.0, 200.0), 1.0);
        assert!(invader_density(120.0, 200.0) < invader_density(60.0, 200.0));
        assert_eq!(invader_density(200.0, 200.0), 0.0);
    }

    #[test]
    fn shot_is_spent_at_the_formation() {
        let (_, y) = shot_position(900.0, 200.0, 0.1).expect("just fired");
        assert!(y < 200.0 - SHIP_OFFSET);
        assert_eq!(shot_position(900.0, 200.0, SHOT_EVERY - 0.01), None);
    }

    #[test]
    fn invader_sprites_are_eleven_by_eight() {
        for frame in INVADER {
            assert!(frame.iter().all(|row| row.len() == 11));
        }
    }

    #[test]
    fn bug_eats_pellets_it_has_passed() {
        let start = trail_frame(240.0, 0.0);
        assert_eq!(start.first_pellet, 0);
        let later = trail_frame(240.0, 1.5);
        assert!(later.first_pellet > 0 && later.first_pellet < later.pellets);
        // Off the far edge every pellet is gone; the next lap restores them.
        let span_secs = (240.0 + BUG_W * 2.0) / BUG_SPEED;
        assert_eq!(
            trail_frame(240.0, span_secs - 0.01).first_pellet,
            start.pellets
        );
        assert_eq!(trail_frame(240.0, span_secs + 0.01).first_pellet, 0);
    }

    #[test]
    fn bug_sprite_frames_are_seven_by_five() {
        for frame in BUG {
            assert!(frame.iter().all(|row| row.len() == 7));
        }
    }
}
