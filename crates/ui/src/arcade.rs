//! Arcade accents: small self-playing pixel scenes that give quiet surfaces
//! some life without competing with the content around them.
//!
//! - [`invaders_header`]: a dim band for the top of the new-session screen.
//!   Rows of invaders march above a sliding ship and dissolve toward the
//!   bottom, so it reads as texture, not a game.
//! - [`Runner`]: a side-scroller in the strip above the composer while an agent
//!   works. It plays itself until the user clicks to take over.
//!
//! Both are grayscale tints, repaint from their own cached view on the shared
//! pulse clock, and freeze on one frame under reduced motion. Scene math lives
//! in pure code so it is unit-tested.

use std::sync::atomic::{AtomicU32, Ordering};

use gpui::prelude::*;
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Hsla, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement, Render, RenderOnce, SharedString, Styled, WeakEntity, Window,
    canvas, div, point, px, size,
};

use crate::composer::Composer;
use crate::motion::{self, MotionSpec};
use crate::theme::Theme;

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
// Runner
// ---------------------------------------------------------------------------

/// Fits the status strip reserved above the composer, so the layout never moves.
pub const RUNNER_HEIGHT: f32 = 22.0;
const RUNNER_X: f32 = 18.0;
const RUN_SPEED: f32 = 110.0;
const RUN_SPEED_MAX: f32 = 230.0;
const RUN_ACCEL: f32 = 0.012;
/// A hop just under half a second, clearing the 8px obstacles inside the strip.
const GRAVITY: f32 = 384.0;
const JUMP_SPEED: f32 = 88.0;
const AUTOPILOT_LEAD_SECS: f32 = 0.2;
/// Obstacles exist this far ahead, wider than any composer.
const SPAWN_HORIZON: f32 = 1_600.0;
const FIRST_OBSTACLE: f32 = 220.0;
const GROUND_GAP: f32 = 7.0;

const SCIENTIST: [[&str; 9]; 2] = [
    [
        "#..#..#", ".#####.", ".#.#.#.", ".#####.", "..###..", ".#####.", "#.###.#", "..#.#..",
        ".##.#..",
    ],
    [
        "#..#..#", ".#####.", ".#.#.#.", ".#####.", "..###..", ".#####.", "#.###.#", "..#.#..",
        "..#.##.",
    ],
];
const SCIENTIST_H: f32 = 9.0;
const FLASK: [&str; 8] = [
    "..###..", "...#...", "...#...", "..#.#..", ".#...#.", ".#.#.#.", "#.###.#", "#######",
];
const COIL: [&str; 8] = [
    "#.#.#", ".###.", "..#..", ".###.", "..#..", ".###.", "..#..", "#####",
];
const OBSTACLES: [&[&str]; 2] = [&FLASK, &COIL];

/// Best score this session, shared by every chat's strip.
static BEST_SCORE: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy)]
struct Obstacle {
    at: f32,
    kind: usize,
}

impl Obstacle {
    fn width(self) -> f32 {
        OBSTACLES[self.kind][0].len() as f32
    }
    fn height(self) -> f32 {
        OBSTACLES[self.kind].len() as f32
    }
}

/// The game itself, free of rendering. Heights grow upward from the ground and
/// an obstacle's `at` is measured along the track.
#[derive(Clone)]
struct RunnerGame {
    distance: f32,
    height: f32,
    velocity: f32,
    obstacles: Vec<Obstacle>,
    next_obstacle: f32,
    seed: u32,
    player: bool,
    over: bool,
}

impl RunnerGame {
    fn new(seed: u32) -> Self {
        let mut game = Self {
            distance: 0.0,
            height: 0.0,
            velocity: 0.0,
            obstacles: Vec::new(),
            next_obstacle: FIRST_OBSTACLE,
            seed: seed.max(1),
            player: false,
            over: false,
        };
        game.spawn();
        game
    }

    fn random(&mut self) -> u32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed
    }

    fn speed(&self) -> f32 {
        (RUN_SPEED + self.distance * RUN_ACCEL).min(RUN_SPEED_MAX)
    }

    fn score(&self) -> u32 {
        (self.distance / 10.0) as u32
    }

    fn grounded(&self) -> bool {
        self.height <= 0.0 && self.velocity <= 0.0
    }

    fn in_play(&self) -> bool {
        self.player && !self.over
    }

    fn spawn(&mut self) {
        while self.next_obstacle < self.distance + SPAWN_HORIZON {
            let kind = self.random() as usize % OBSTACLES.len();
            self.obstacles.push(Obstacle {
                at: self.next_obstacle,
                kind,
            });
            // At least a full hop apart, so every gap is clearable at speed.
            let gap = self.speed() * 0.7 + (self.random() % 90) as f32;
            self.next_obstacle += gap;
        }
    }

    fn jump(&mut self) {
        if !self.over && self.grounded() {
            self.velocity = JUMP_SPEED;
        }
    }

    fn take_over(&mut self) {
        *self = Self::new(self.seed);
        self.player = true;
    }

    fn release(&mut self) {
        *self = Self::new(self.seed);
    }

    /// Jump, or start again after a crash.
    fn press(&mut self) {
        if self.over {
            self.take_over();
        } else {
            self.jump();
        }
    }

    fn collided(&self) -> bool {
        self.obstacles.iter().any(|obstacle| {
            let x = obstacle.at - self.distance;
            x < 6.0 && x + obstacle.width() > 1.0 && self.height < obstacle.height() - 1.0
        })
    }

    fn tick(&mut self, dt: f32) {
        if self.over {
            return;
        }
        self.distance += self.speed() * dt;
        if !self.grounded() {
            self.height += self.velocity * dt;
            self.velocity -= GRAVITY * dt;
            if self.height <= 0.0 {
                self.height = 0.0;
                self.velocity = 0.0;
            }
        }
        let distance = self.distance;
        self.obstacles
            .retain(|obstacle| obstacle.at + obstacle.width() + RUNNER_X > distance);
        self.spawn();
        if self.player {
            self.over = self.collided();
        } else if self.obstacles.iter().any(|obstacle| {
            let ahead = obstacle.at - self.distance;
            ahead > 0.0 && ahead <= self.speed() * AUTOPILOT_LEAD_SECS
        }) {
            self.jump();
        }
    }
}

fn paint_mono(
    origin: gpui::Point<gpui::Pixels>,
    sprite: &[&str],
    (x, y): (f32, f32),
    color: Hsla,
    window: &mut Window,
) {
    for (row, line) in sprite.iter().enumerate() {
        for (col, cell) in line.bytes().enumerate() {
            if cell == b'#' {
                fill(origin, x + col as f32, y + row as f32, 1.0, color, window);
            }
        }
    }
}

/// The strip's view: owns the game and the keyboard while the user plays.
pub struct Runner {
    game: RunnerGame,
    focus: FocusHandle,
    owner: WeakEntity<Composer>,
    composer_focus: FocusHandle,
    agent_working: bool,
    last_tick: Option<f32>,
}

impl Runner {
    pub fn new(
        owner: WeakEntity<Composer>,
        composer_focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            game: RunnerGame::new(0x9E37_79B9),
            focus: cx.focus_handle(),
            owner,
            composer_focus,
            agent_working: false,
            last_tick: None,
        }
    }

    /// Whether the strip should be on screen. A run in progress outlives the
    /// agent's turn; once it ends, the keyboard goes back to the composer.
    pub fn mounted(
        &mut self,
        agent_working: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.agent_working = agent_working;
        let mounted = agent_working || self.game.in_play();
        if !mounted {
            self.game.release();
            self.last_tick = None;
            if self.focus.is_focused(window) {
                window.focus(&self.composer_focus, cx);
            }
        }
        mounted
    }

    /// The composer decides whether the strip stays once the agent is idle.
    fn wake_composer(&self, cx: &mut Context<Self>) {
        if self.agent_working {
            return;
        }
        let owner = self.owner.clone();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |_, cx| cx.notify());
        });
    }

    fn on_mouse_down(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if cx.reduce_motion() {
            return;
        }
        if self.game.player {
            self.game.press();
        } else {
            self.game.take_over();
            window.focus(&self.focus, cx);
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.game.player {
            return;
        }
        match event.keystroke.key.as_str() {
            "space" | "up" => {
                // Holding jump through a crash must not skip the score.
                if !(event.is_held && self.game.over) {
                    self.game.press();
                }
            }
            "escape" => {
                self.game.release();
                window.focus(&self.composer_focus, cx);
                self.wake_composer(cx);
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

/// Mount the runner, [`RUNNER_HEIGHT`] tall and as wide as its parent.
pub fn runner_strip(view: &Entity<Runner>) -> impl IntoElement + use<> {
    view.clone().cached(
        gpui::StyleRefinement::default()
            .w_full()
            .h(px(RUNNER_HEIGHT)),
    )
}

impl Render for Runner {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = motion::pulse_delta(&ARCADE_CLOCK, cx.entity_id(), cx)
            * ARCADE_CLOCK.total().as_secs_f32();
        // Clamped so a stall or the clock's wrap can't teleport the player.
        let dt = self
            .last_tick
            .map_or(0.0, |last| (now - last).clamp(0.0, 0.1));
        self.last_tick = Some(now);
        // Clicking the composer takes the keyboard away, and with it the run.
        if self.game.player && !self.focus.is_focused(window) {
            self.game.release();
            self.wake_composer(cx);
        }
        let was_over = self.game.over;
        self.game.tick(dt);
        if self.game.over && !was_over {
            BEST_SCORE.fetch_max(self.game.score(), Ordering::Relaxed);
            self.wake_composer(cx);
        }

        let theme = Theme::of(cx);
        let tint = theme.text_muted;
        let game = self.game.clone();
        let playing = game.player;
        let alpha = if playing { 0.9 } else { 0.45 };
        let score = playing.then(|| {
            let best = BEST_SCORE.load(Ordering::Relaxed).max(game.score());
            if game.over {
                format!("{}  Space to retry", game.score())
            } else {
                format!("{}  Best {best}", game.score())
            }
        });
        div()
            .id("composer-runner")
            .role(gpui::Role::Button)
            .aria_label("Play while the agent works")
            .relative()
            .w_full()
            .h(px(RUNNER_HEIGHT))
            .cursor_pointer()
            .occlude()
            .track_focus(&self.focus)
            .when(!playing, |d| {
                d.tooltip(crate::settings::widgets::text_tooltip("Click to play"))
            })
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                            let width = f32::from(bounds.size.width);
                            let ground = RUNNER_HEIGHT - 1.0;
                            let mut x = -game.distance.rem_euclid(GROUND_GAP);
                            while x < width {
                                fill(
                                    bounds.origin,
                                    x,
                                    ground,
                                    1.0,
                                    tint.opacity(alpha * 0.5),
                                    window,
                                );
                                x += GROUND_GAP;
                            }
                            for obstacle in &game.obstacles {
                                let x = RUNNER_X + obstacle.at - game.distance;
                                if x < width {
                                    paint_mono(
                                        bounds.origin,
                                        OBSTACLES[obstacle.kind],
                                        (x, ground - obstacle.height()),
                                        tint.opacity(alpha),
                                        window,
                                    );
                                }
                            }
                            let stride = if game.grounded() && !game.over {
                                (game.distance / 14.0) as usize % 2
                            } else {
                                0
                            };
                            paint_mono(
                                bounds.origin,
                                &SCIENTIST[stride],
                                (RUNNER_X, ground - SCIENTIST_H - game.height),
                                tint.opacity(alpha),
                                window,
                            );
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(score.map(|score| {
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .text_size(crate::typography::ui_rems(10.0))
                    .text_color(tint)
                    .child(score)
            }))
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

    const STEP: f32 = 1.0 / 30.0;

    #[test]
    fn autopilot_clears_every_obstacle_at_every_speed() {
        let mut game = RunnerGame::new(7);
        for _ in 0..(30 * 240) {
            game.tick(STEP);
            assert!(!game.collided(), "hit at {}", game.distance);
        }
        assert_eq!(game.speed(), RUN_SPEED_MAX);
    }

    #[test]
    fn an_idle_player_crashes_and_a_press_restarts() {
        let mut game = RunnerGame::new(7);
        game.take_over();
        for _ in 0..(30 * 10) {
            game.tick(STEP);
        }
        assert!(game.over && !game.in_play());
        let score = game.score();
        game.tick(STEP);
        assert_eq!(game.score(), score, "a crashed run stays put");
        game.press();
        assert!(game.in_play() && game.score() == 0);
    }

    #[test]
    fn a_jump_peaks_inside_the_strip_and_lands() {
        let mut game = RunnerGame::new(7);
        game.player = true;
        game.obstacles.clear();
        game.next_obstacle = f32::MAX;
        game.jump();
        let mut peak: f32 = 0.0;
        for _ in 0..30 {
            game.tick(STEP);
            peak = peak.max(game.height);
        }
        assert!(peak > 7.0, "{peak} must clear an obstacle");
        assert!(peak + SCIENTIST_H <= RUNNER_HEIGHT - 1.0, "{peak}");
        assert!(game.grounded());
    }

    #[test]
    fn jumping_needs_the_ground() {
        let mut game = RunnerGame::new(7);
        game.jump();
        game.tick(STEP);
        let rising = game.velocity;
        game.jump();
        assert_eq!(game.velocity, rising);
    }

    #[test]
    fn runner_sprites_are_rectangular() {
        for sprite in SCIENTIST.iter().map(|s| &s[..]).chain(OBSTACLES) {
            assert!(sprite.iter().all(|row| row.len() == sprite[0].len()));
        }
    }
}
