//! Zapper avatars, shared by every client: soft pixel mascots that turn
//! toward you.
//!
//! Each mascot is a [`Body`]: a flat-colour blob (a superellipse, optionally
//! tapered into a drop) with big capsule or oval eyes. A [`Pose`] turns it in
//! fake 3D: the eyes ride the curved front of the body, sliding and
//! foreshortening as it yaws and pitches, while roll, lift and squash move
//! the silhouette. [`raster`] samples the posed mascot onto a coarse grid,
//! so it reads as pixel art at every size. [`idle_pose`] supplies the life:
//! a slow ambient sway, irregular blinks, small eye darts, and a head that
//! follows a look target (the pointer), and a [`Mood`] that shows what the
//! Zapper is doing: resting with closed eyes, wide awake while it works,
//! eyes scanning while only its threads work, an attention hop when it needs
//! you. Mascot and colour ids are saved with
//! each Zapper, so art and names can change but ids cannot; every id saved
//! by an earlier version still names one of these.
use crate::zots::ZotAvatar;

/// The flat round face an earlier version drew for each mascot. The Slack
/// persona renderer still draws these until it learns the sprites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// Round eyes and a small smile.
    Smile,
    /// Tall pill eyes, no mouth.
    Tall,
    /// One wide visor with two glints.
    Visor,
    /// A single large eye.
    Cyclops,
    /// Closed, smiling eyes.
    Happy,
    /// Square shades and a flat mouth.
    Shades,
    /// One open eye, one closed, a sideways smirk.
    Wink,
    /// Closed, resting eyes and a small round mouth.
    Sleepy,
    /// Round eyes, rosy cheeks and a button mouth.
    Blush,
}

/// An eye's outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EyeShape {
    /// A pill: rounded ends on the long axis.
    Capsule,
    /// An ellipse.
    Oval,
}

/// One eye, in avatar units (the avatar spans -1..1 on both axes) relative
/// to the body's centre: centre `x`, `y`, full width `w` and height `h`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Eye {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub shape: EyeShape,
}

/// How often a mascot blinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Temper {
    Calm,
    Normal,
    Bright,
    Sleepy,
}

impl Temper {
    /// Gaps between blinks in ms, cycled, so blinks never feel metronomic.
    fn intervals(self) -> &'static [u64] {
        match self {
            Temper::Calm => &[6800, 5300, 7900, 6100],
            Temper::Normal => &[5100, 7200, 4300, 6400, 5600],
            Temper::Bright => &[3900, 5700, 4500, 6900],
            Temper::Sleepy => &[7600, 9100, 6800],
        }
    }

    fn blink_ms(self) -> u64 {
        if self == Temper::Sleepy { 320 } else { 205 }
    }
}

/// A mascot's body, in avatar units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    /// Half width and half height.
    pub a: f32,
    pub b: f32,
    /// Superellipse exponent: 2 is an ellipse, higher is squarer.
    pub n: f32,
    /// Narrows the top into a drop: 0 is none, 0.4 is a clear point.
    pub taper: f32,
    /// Body centre below the avatar centre, leaving room to sway.
    pub dy: f32,
    pub eyes: &'static [Eye],
    pub temper: Temper,
}

pub struct Mascot {
    pub id: &'static str,
    pub name: &'static str,
    /// One line for tooltips and accessibility.
    pub about: &'static str,
    pub face: Face,
    pub body: Body,
}

const fn capsule(x: f32, y: f32, w: f32, h: f32) -> Eye {
    Eye {
        x,
        y,
        w,
        h,
        shape: EyeShape::Capsule,
    }
}

const fn oval(x: f32, y: f32, w: f32, h: f32) -> Eye {
    Eye {
        x,
        y,
        w,
        h,
        shape: EyeShape::Oval,
    }
}

pub const MASCOTS: &[Mascot] = &[
    Mascot {
        id: "bolt",
        name: "Pebble",
        about: "Round and steady, with tall pill eyes",
        face: Face::Smile,
        body: Body {
            a: 0.70,
            b: 0.64,
            n: 2.4,
            taper: 0.,
            dy: 0.06,
            eyes: &[
                capsule(-0.21, -0.06, 0.18, 0.38),
                capsule(0.21, -0.06, 0.18, 0.38),
            ],
            temper: Temper::Normal,
        },
    },
    Mascot {
        id: "ace",
        name: "Comet",
        about: "A bright drop that blinks a lot",
        face: Face::Tall,
        body: Body {
            a: 0.64,
            b: 0.72,
            n: 2.2,
            taper: 0.38,
            dy: 0.04,
            eyes: &[
                capsule(-0.18, 0.06, 0.17, 0.34),
                capsule(0.18, 0.06, 0.17, 0.34),
            ],
            temper: Temper::Bright,
        },
    },
    Mascot {
        id: "brawler",
        name: "Brick",
        about: "A squared-off block with steady eyes",
        face: Face::Visor,
        body: Body {
            a: 0.76,
            b: 0.62,
            n: 3.6,
            taper: 0.,
            dy: 0.08,
            eyes: &[
                capsule(-0.22, -0.04, 0.2, 0.3),
                capsule(0.22, -0.04, 0.2, 0.3),
            ],
            temper: Temper::Calm,
        },
    },
    Mascot {
        id: "hex",
        name: "Orbit",
        about: "A perfect circle with close-set round eyes",
        face: Face::Cyclops,
        body: Body {
            a: 0.68,
            b: 0.68,
            n: 2.,
            taper: 0.,
            dy: 0.06,
            eyes: &[oval(-0.15, -0.04, 0.2, 0.24), oval(0.15, -0.04, 0.2, 0.24)],
            temper: Temper::Normal,
        },
    },
    Mascot {
        id: "squire",
        name: "Bean",
        about: "Tall and curious, eyes set high",
        face: Face::Happy,
        body: Body {
            a: 0.56,
            b: 0.80,
            n: 2.5,
            taper: 0.,
            dy: 0.04,
            eyes: &[oval(-0.17, -0.16, 0.2, 0.3), oval(0.17, -0.16, 0.2, 0.3)],
            temper: Temper::Bright,
        },
    },
    Mascot {
        id: "shade",
        name: "Slab",
        about: "Wide and low, with broad eyes",
        face: Face::Shades,
        body: Body {
            a: 0.82,
            b: 0.54,
            n: 3.2,
            taper: 0.,
            dy: 0.10,
            eyes: &[
                capsule(-0.25, -0.02, 0.26, 0.22),
                capsule(0.25, -0.02, 0.26, 0.22),
            ],
            temper: Temper::Calm,
        },
    },
    Mascot {
        id: "rogue",
        name: "Wink",
        about: "One tall eye and one round one",
        face: Face::Wink,
        body: Body {
            a: 0.70,
            b: 0.66,
            n: 2.6,
            taper: 0.,
            dy: 0.06,
            eyes: &[
                capsule(-0.21, -0.06, 0.18, 0.38),
                oval(0.22, 0., 0.22, 0.22),
            ],
            temper: Temper::Normal,
        },
    },
    Mascot {
        id: "viking",
        name: "Doze",
        about: "Broad, soft and half asleep",
        face: Face::Sleepy,
        body: Body {
            a: 0.80,
            b: 0.64,
            n: 2.8,
            taper: 0.,
            dy: 0.08,
            eyes: &[
                capsule(-0.23, 0.04, 0.28, 0.12),
                capsule(0.23, 0.04, 0.28, 0.12),
            ],
            temper: Temper::Sleepy,
        },
    },
    Mascot {
        id: "bones",
        name: "Rosy",
        about: "A round face with friendly oval eyes",
        face: Face::Blush,
        body: Body {
            a: 0.66,
            b: 0.70,
            n: 2.2,
            taper: 0.,
            dy: 0.06,
            eyes: &[oval(-0.2, -0.08, 0.2, 0.3), oval(0.2, -0.08, 0.2, 0.3)],
            temper: Temper::Calm,
        },
    },
];

pub struct MascotColor {
    pub id: &'static str,
    pub body: u32,
    pub shade: u32,
}

pub const COLORS: &[MascotColor] = &[
    MascotColor {
        id: "coral",
        body: 0xef6b5f,
        shade: 0xc94a45,
    },
    MascotColor {
        id: "yellow",
        body: 0xf6c94a,
        shade: 0xe0a92c,
    },
    MascotColor {
        id: "green",
        body: 0x7fd05a,
        shade: 0x5aae3c,
    },
    MascotColor {
        id: "lime",
        body: 0xc3e45a,
        shade: 0x9ec43a,
    },
    MascotColor {
        id: "violet",
        body: 0xa98bf5,
        shade: 0x8263dc,
    },
    MascotColor {
        id: "teal",
        body: 0x5fd0c0,
        shade: 0x36ab9c,
    },
    MascotColor {
        id: "blue",
        body: 0x6ea8ff,
        shade: 0x4b82e0,
    },
    MascotColor {
        id: "pink",
        body: 0xff8fbf,
        shade: 0xe2669c,
    },
];

fn hash(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

/// A stable avatar for a seed, so Zots saved without one keep their look.
pub fn default_avatar(seed: &str) -> ZotAvatar {
    let value = hash(seed);
    ZotAvatar {
        mascot: MASCOTS[value as usize % MASCOTS.len()].id.into(),
        color: COLORS[(value >> 8) as usize % COLORS.len()].id.into(),
    }
}

/// The mascot and colour an avatar draws, unknown ids falling back to the
/// first of each so a Zapper saved by a newer client still has a face.
pub fn resolve(avatar: &ZotAvatar) -> (&'static Mascot, &'static MascotColor) {
    let mascot = MASCOTS
        .iter()
        .find(|m| m.id == avatar.mascot)
        .unwrap_or(&MASCOTS[0]);
    let color = COLORS
        .iter()
        .find(|c| c.id == avatar.color)
        .unwrap_or(&COLORS[0]);
    (mascot, color)
}

/// The eye colour on every body colour.
pub const EYE: u32 = 0x18181c;

/// Where a mascot is looking and how it is held, in degrees and avatar units.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pose {
    /// Turn: positive faces right.
    pub yaw: f32,
    /// Nod: positive faces down.
    pub pitch: f32,
    /// Tilt: positive leans clockwise.
    pub roll: f32,
    /// Vertical offset: positive is lower.
    pub lift: f32,
    /// 0 is at rest; positive squashes, negative stretches.
    pub squash: f32,
    /// Eye offset across the face.
    pub gaze: (f32, f32),
    /// 0 is open, 1 is shut.
    pub blink: f32,
    /// Extra eye size: 0 is normal, 0.25 is a quarter larger.
    pub wide: f32,
}

/// What a Zapper is doing, as its mascot shows it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Mood {
    /// Nothing known: awake and looking around.
    #[default]
    Awake,
    /// Nothing running: eyes closed, breathing slowly.
    Resting,
    /// The Zapper itself is working: wide eyes and a lively bounce.
    Working,
    /// Only its threads are working: eyes scanning back and forth.
    Delegating,
    /// Waiting on you: wide eyes on the pointer and a hop now and then.
    NeedsYou,
}

/// The largest turn toward a look target.
pub const LOOK_YAW: f32 = 12.;
pub const LOOK_PITCH: f32 = 8.;
/// How quickly eyes and head catch up with a look target, in ms.
pub const EYES_FOLLOW_MS: f32 = 72.;
pub const HEAD_FOLLOW_MS: f32 = 190.;

fn ease_quick(t: f32) -> f32 {
    1. - (1. - t).powi(4)
}

fn ease_gentle(t: f32) -> f32 {
    t * t * t * (t * (6. * t - 15.) + 10.)
}

/// How shut the eyes are at `ms`: blinks fall at the temper's cycled gaps,
/// close quickly, hold a moment and reopen gently.
pub fn blink_at(mascot: &Mascot, ms: u64) -> f32 {
    let temper = mascot.body.temper;
    let gaps = temper.intervals();
    let cycle: u64 = gaps.iter().sum();
    // Each mascot starts at its own point in the cycle.
    let t = (ms + u64::from(hash(mascot.id)) % cycle) % cycle;
    let mut end = 0;
    for gap in gaps {
        end += gap;
        let length = temper.blink_ms().min(*gap);
        if (end - length..end).contains(&t) {
            let p = (t - (end - length)) as f32 / length as f32;
            return if p < 0.23 {
                ease_quick(p / 0.23)
            } else if p < 0.39 {
                1.
            } else {
                1. - ease_gentle((p - 0.39) / 0.61)
            };
        }
    }
    0.
}

/// The pose at `ms` in `mood`, turned by `eyes` and `head`: smoothed look
/// targets in -1..1, positive toward the right and down.
pub fn idle_pose(mascot: &Mascot, mood: Mood, ms: u64, eyes: (f32, f32), head: (f32, f32)) -> Pose {
    let s = (ms as f32 / 1000.) + (hash(mascot.id) % 1000) as f32 / 37.;
    let sway = |rate: f32, phase: f32| (s * rate + phase).sin();
    if mood == Mood::Resting {
        // Asleep: eyes shut, a slow breath, the head dipped a little.
        let breath = sway(1.4, 0.);
        return Pose {
            pitch: 4. + sway(0.41, 2.3) * 1.5,
            roll: sway(0.53, 1.1) * 1.2,
            lift: breath * 0.025,
            squash: breath * 0.03,
            blink: 1.,
            ..Pose::default()
        };
    }
    // Small eye darts every 1.24s between a few fixed points.
    const DARTS: [(f32, f32); 6] = [
        (0., 0.),
        (0.6, -0.2),
        (-0.4, 0.3),
        (0.2, 0.5),
        (-0.7, -0.1),
        (0.3, 0.1),
    ];
    let step = (ms / 1240) as usize;
    let travel = ((ms % 1240) as f32 / 125.).min(1.);
    let (from, to) = (DARTS[step % 6], DARTS[(step + 1) % 6]);
    let dart = (
        from.0 + (to.0 - from.0) * ease_gentle(travel),
        from.1 + (to.1 - from.1) * ease_gentle(travel),
    );
    let mut pose = Pose {
        yaw: sway(0.73, 0.) * 3. + head.0 * LOOK_YAW,
        pitch: sway(0.41, 2.3) * 2.5 + head.1 * LOOK_PITCH,
        roll: sway(0.53, 1.1) * 1.6 + head.0 * head.1 * 2.2,
        lift: sway(0.79, 0.4) * 0.035,
        squash: sway(1.06, 0.) * 0.02,
        gaze: (
            sway(0.29, 0.7) * 0.02 + dart.0 * 0.015 + eyes.0 * 0.08,
            sway(0.23, 2.7) * 0.015 + dart.1 * 0.01 + eyes.1 * 0.06,
        ),
        blink: blink_at(mascot, ms),
        wide: 0.,
    };
    match mood {
        Mood::Working => {
            // Busy: eyes wide, a quick light bounce.
            let bounce = sway(5.2, 0.).abs();
            pose.wide = 0.2;
            pose.lift = -bounce * 0.05;
            pose.squash = (1. - bounce) * 0.04 - 0.01;
        }
        Mood::Delegating => {
            // Watching its threads: eyes sweep side to side, the head trails.
            let sweep = sway(2.2, 0.);
            pose.gaze.0 = sweep * 0.08 + eyes.0 * 0.03;
            pose.yaw = sway(2.2, -0.6) * 5. + head.0 * LOOK_YAW * 0.4;
        }
        Mood::NeedsYou => {
            // Calling for you: wide eyes on the pointer, a hop every 2.2s.
            pose.wide = 0.25;
            pose.gaze = (eyes.0 * 0.11, eyes.1 * 0.08);
            let p = (ms % 2200) as f32 / 2200.;
            if p < 0.16 {
                let hop = (p / 0.16 * std::f32::consts::PI).sin();
                pose.lift = pose.lift * 0.2 - hop * 0.08;
                pose.squash -= hop * 0.04;
                pose.blink = 0.;
            }
        }
        Mood::Awake | Mood::Resting => {}
    }
    pose
}

/// A still pose that still says what the Zapper is doing, for reduced motion.
pub fn still_pose(mood: Mood) -> Pose {
    match mood {
        Mood::Awake => Pose::default(),
        Mood::Resting => Pose {
            pitch: 4.,
            blink: 1.,
            ..Pose::default()
        },
        Mood::Working => Pose {
            wide: 0.2,
            ..Pose::default()
        },
        Mood::Delegating => Pose {
            yaw: 4.,
            gaze: (0.07, 0.),
            ..Pose::default()
        },
        Mood::NeedsYou => Pose {
            wide: 0.25,
            pitch: -5.,
            ..Pose::default()
        },
    }
}

/// A grid cell of a drawn mascot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cell {
    Body,
    Eye,
}

/// Cells across for an avatar `size` logical pixels wide.
pub fn grid_for(size: f32) -> usize {
    // About one and a half logical pixels per cell: pixel art, but fine.
    (((size / 1.5).round() as usize).clamp(16, 44) / 2) * 2
}

/// Mascots are modelled with room to sway, then drawn this much larger so
/// they fill their tile.
const ZOOM: f32 = 1.16;

/// `mascot` in `pose` on a `grid` x `grid` raster, row by row.
pub fn raster(mascot: &Mascot, grid: usize, pose: Pose) -> Vec<Option<Cell>> {
    let body = mascot.body;
    let (yaw, pitch, roll) = (
        pose.yaw.to_radians(),
        pose.pitch.to_radians(),
        pose.roll.to_radians(),
    );
    let g = grid as f32;
    let depth = 0.9 * body.a.min(body.b);
    // A turned body looks a touch narrower.
    let a = body.a * (1. - 0.08 * yaw.sin().powi(2));
    let centre_y = body.dy + pose.lift;
    let inside = |qx: f32, qy: f32| {
        let along = (qy / body.b).clamp(-1., 1.);
        let half = a * (1. - body.taper * (0.5 - 0.5 * along));
        (qx / half).abs().powf(body.n) + (qy / body.b).abs().powf(body.n) <= 1.
    };
    let mut out = vec![None; grid * grid];
    let (sin, cos) = (-roll).sin_cos();
    for y in 0..grid {
        for x in 0..grid {
            let px = ((x as f32 + 0.5) / g * 2. - 1.) / ZOOM;
            let py = ((y as f32 + 0.5) / g * 2. - 1.) / ZOOM - centre_y;
            let (qx, qy) = (px * cos - py * sin, px * sin + py * cos);
            let (qx, qy) = (qx / (1. + pose.squash * 0.5), qy / (1. - pose.squash));
            if inside(qx, qy) {
                out[y * grid + x] = Some(Cell::Body);
            }
        }
    }
    let bodies: Vec<bool> = out.iter().map(Option::is_some).collect();
    let min_side = 2. / g * 1.05 / ZOOM;
    for eye in body.eyes {
        // The eye sits on the front of an ellipsoid; turn it with the head.
        let (u, v) = (eye.x / body.a, eye.y / body.b);
        let z0 = depth * (1. - u * u - v * v).max(0.05).sqrt();
        let (x1, z1) = (
            eye.x * yaw.cos() + z0 * yaw.sin(),
            -eye.x * yaw.sin() + z0 * yaw.cos(),
        );
        let (y2, z2) = (
            eye.y * pitch.cos() + z1 * pitch.sin(),
            -eye.y * pitch.sin() + z1 * pitch.cos(),
        );
        // Past the rim it is out of sight.
        if z2 < 0.12 * depth {
            continue;
        }
        let squeeze = |turned: f32, rest: f32| (turned.cos() / rest.cos()).clamp(0.6, 1.2);
        let grow = 1. + pose.wide;
        let shut = pose.blink > 0.8;
        // Closed eyes are a line; a slit, already a line, narrows instead.
        let slit = eye.h * ZOOM * g / 2. < 2.5;
        let w = (eye.w
            * grow
            * squeeze(x1.atan2(z1), eye.x.atan2(z0))
            * if shut && slit { 0.7 } else { 1. })
        .max(min_side);
        let h = (eye.h * grow * squeeze(y2.atan2(z2), eye.y.atan2(z0)) * (1. - pose.blink))
            .max(min_side);
        // Whole-cell boxes keep eyes crisp; trimmed corners round them.
        let cx = ((x1 + pose.gaze.0) * ZOOM + 1.) / 2. * g;
        // An eye centred on a grid line needs an even width, one centred in
        // a cell an odd one, or it lands a cell off and the face goes lopsided.
        let mut wc = ((w * ZOOM * g / 2.).round() as isize).max(1);
        if wc % 2 != ((cx * 2.).round() as isize).rem_euclid(2) {
            wc += if (w * ZOOM * g / 2.) > wc as f32 {
                1
            } else {
                -1
            };
            wc = wc.max(1);
        }
        let hc = ((h * ZOOM * g / 2.).round() as isize).max(1);
        let cy = ((y2 + pose.gaze.1 + centre_y) * ZOOM + 1.) / 2. * g;
        let x0 = (cx - wc as f32 / 2. + 0.5).floor() as isize;
        let y0 = (cy - hc as f32 / 2. + 0.5).floor() as isize;
        let round = wc >= 3 && hc >= 3 && wc + hc >= 7;
        for yy in y0..y0 + hc {
            for xx in x0..x0 + wc {
                let corner = (xx == x0 || xx == x0 + wc - 1) && (yy == y0 || yy == y0 + hc - 1);
                if (corner && round)
                    || xx < 0
                    || yy < 0
                    || xx >= grid as isize
                    || yy >= grid as isize
                {
                    continue;
                }
                // Keep a ring of body around every eye cell, so an eye near
                // the rim never bites into the silhouette.
                let solid = |x: isize, y: isize| {
                    (0..grid as isize).contains(&x)
                        && (0..grid as isize).contains(&y)
                        && bodies[y as usize * grid + x as usize]
                };
                if [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .all(|(dx, dy)| solid(xx + dx, yy + dy))
                {
                    out[yy as usize * grid + xx as usize] = Some(Cell::Eye);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mascot_ids_are_unique() {
        let mut ids: Vec<_> = MASCOTS.iter().map(|m| m.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), MASCOTS.len());
    }

    #[test]
    fn defaults_are_stable_and_unknown_ids_still_draw() {
        assert_eq!(default_avatar("zot"), default_avatar("zot"));
        let unknown = ZotAvatar {
            mascot: "gone".into(),
            color: "gone".into(),
        };
        let (mascot, color) = resolve(&unknown);
        assert_eq!((mascot.id, color.id), (MASCOTS[0].id, COLORS[0].id));
    }

    /// Every mascot id an earlier version could save, and the robot it now
    /// draws. Ids are durable; this list may only grow.
    const LEGACY: [(&str, &str); 9] = [
        ("bolt", "Pebble"),
        ("ace", "Comet"),
        ("brawler", "Brick"),
        ("hex", "Orbit"),
        ("squire", "Bean"),
        ("shade", "Slab"),
        ("rogue", "Wink"),
        ("viking", "Doze"),
        ("bones", "Rosy"),
    ];

    #[test]
    fn every_saved_mascot_id_maps_to_its_mascot() {
        for (id, name) in LEGACY {
            let avatar = ZotAvatar {
                mascot: id.into(),
                color: "teal".into(),
            };
            assert_eq!(resolve(&avatar).0.name, name, "{id}");
        }
        for color in [
            "coral", "yellow", "green", "lime", "violet", "teal", "blue", "pink",
        ] {
            assert!(COLORS.iter().any(|c| c.id == color), "{color}");
        }
    }

    fn count(cells: &[Option<Cell>], kind: Cell) -> usize {
        cells.iter().filter(|c| **c == Some(kind)).count()
    }

    /// Mean column and row of the eye cells.
    fn eyes_at(cells: &[Option<Cell>], grid: usize) -> (f32, f32) {
        let eyes: Vec<_> = (0..cells.len())
            .filter(|i| cells[*i] == Some(Cell::Eye))
            .collect();
        let n = eyes.len() as f32;
        (
            eyes.iter().map(|i| (i % grid) as f32).sum::<f32>() / n,
            eyes.iter().map(|i| (i / grid) as f32).sum::<f32>() / n,
        )
    }

    #[test]
    fn mascots_fill_the_grid_show_their_eyes_and_differ() {
        for grid in [16, 26, 42] {
            let mut seen = std::collections::HashSet::new();
            for mascot in MASCOTS {
                assert!(!mascot.about.is_empty());
                let cells = raster(mascot, grid, Pose::default());
                assert!(
                    seen.insert(cells.clone()),
                    "{} repeats at {grid}",
                    mascot.id
                );
                let filled = cells.iter().filter(|c| c.is_some()).count();
                assert!(
                    filled > grid * grid * 35 / 100,
                    "{} too small at {grid}",
                    mascot.id
                );
                assert!(
                    count(&cells, Cell::Eye) >= 2,
                    "{} has no eyes at {grid}",
                    mascot.id
                );
                // Symmetric faces come out symmetric at rest.
                if mascot.id != "rogue" {
                    for y in 0..grid {
                        for x in 0..grid {
                            assert_eq!(
                                cells[y * grid + x],
                                cells[y * grid + grid - 1 - x],
                                "{} is lopsided at {grid}",
                                mascot.id
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn turning_moves_the_eyes_across_the_face() {
        for mascot in MASCOTS {
            let grid = 42;
            let rest = eyes_at(&raster(mascot, grid, Pose::default()), grid);
            let right = Pose {
                yaw: LOOK_YAW,
                ..Pose::default()
            };
            let down = Pose {
                pitch: LOOK_PITCH,
                ..Pose::default()
            };
            assert!(
                eyes_at(&raster(mascot, grid, right), grid).0 > rest.0,
                "{}",
                mascot.id
            );
            assert!(
                eyes_at(&raster(mascot, grid, down), grid).1 > rest.1,
                "{}",
                mascot.id
            );
            // Eyes never leave the body, even at the widest turn and tilt.
            let far = Pose {
                yaw: -LOOK_YAW - 3.,
                pitch: -LOOK_PITCH - 3.,
                roll: 4.,
                lift: 0.035,
                ..Pose::default()
            };
            for grid in [16, 26, 42] {
                let cells = raster(mascot, grid, far);
                assert!(count(&cells, Cell::Eye) > 0, "{}", mascot.id);
                // Eyes stay inside a ring of body: no bites out of the outline.
                for (i, cell) in cells.iter().enumerate() {
                    if *cell == Some(Cell::Eye) {
                        let (x, y) = ((i % grid) as isize, (i / grid) as isize);
                        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                            let (nx, ny) = (x + dx, y + dy);
                            assert!(
                                cells[(ny as usize) * grid + nx as usize].is_some(),
                                "{}",
                                mascot.id
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn blinks_are_brief_irregular_and_shut_the_eyes() {
        for mascot in MASCOTS {
            let samples: Vec<f32> = (0..60_000)
                .step_by(10)
                .map(|ms| blink_at(mascot, ms))
                .collect();
            assert!(samples.iter().all(|b| (0. ..=1.).contains(b)));
            assert!(
                samples.iter().any(|b| *b > 0.99),
                "{} never shuts",
                mascot.id
            );
            let shut = samples.iter().filter(|b| **b > 0.05).count() as f32 / samples.len() as f32;
            assert!(shut < 0.08, "{} blinks too much: {shut}", mascot.id);
            let open = raster(mascot, 42, Pose::default());
            let closed = raster(
                mascot,
                42,
                Pose {
                    blink: 1.,
                    ..Pose::default()
                },
            );
            assert!(
                count(&closed, Cell::Eye) < count(&open, Cell::Eye),
                "{}",
                mascot.id
            );
        }
        // Blinks start at different moments for different mascots.
        let first = |m: &Mascot| (0..20_000).find(|ms| blink_at(m, *ms) > 0.5);
        let starts: std::collections::HashSet<_> = MASCOTS.iter().map(first).collect();
        assert!(starts.len() > 3);
    }

    #[test]
    fn idle_pose_is_subtle_and_follows_the_look_target() {
        let mascot = &MASCOTS[0];
        for ms in (0..30_000).step_by(97) {
            let pose = idle_pose(mascot, Mood::Awake, ms, (0., 0.), (0., 0.));
            assert!(pose.yaw.abs() <= 3. && pose.pitch.abs() <= 2.5 && pose.roll.abs() <= 1.6);
            assert!(pose.lift.abs() <= 0.035 && pose.squash.abs() <= 0.02);
        }
        let looking = idle_pose(mascot, Mood::Awake, 0, (1., 1.), (1., 1.));
        let ahead = idle_pose(mascot, Mood::Awake, 0, (0., 0.), (0., 0.));
        assert!(looking.yaw - ahead.yaw > LOOK_YAW - 0.01);
        assert!(looking.pitch - ahead.pitch > LOOK_PITCH - 0.01);
        assert!(looking.gaze.0 > ahead.gaze.0 && looking.gaze.1 > ahead.gaze.1);
        assert_eq!(grid_for(16.), 16);
        assert_eq!(grid_for(40.), 26);
        assert_eq!(grid_for(64.), 42);
    }

    /// Rows that hold eye cells.
    fn eye_rows(cells: &[Option<Cell>], grid: usize) -> usize {
        (0..grid)
            .filter(|y| (0..grid).any(|x| cells[y * grid + x] == Some(Cell::Eye)))
            .count()
    }

    #[test]
    fn moods_read_at_a_glance() {
        let grid = 42;
        for mascot in MASCOTS {
            let at = |mood: Mood, ms: u64| idle_pose(mascot, mood, ms, (0., 0.), (0., 0.));
            // Resting: eyes are closed lines the whole time.
            for ms in (0..20_000).step_by(250) {
                let cells = raster(mascot, grid, at(Mood::Resting, ms));
                assert!(eye_rows(&cells, grid) <= 2, "{} opens its eyes", mascot.id);
                assert!(count(&cells, Cell::Eye) > 0, "{} loses its eyes", mascot.id);
            }
            // Working: wider eyes than plain awake, and it bounces.
            let open = |mood| {
                raster(
                    mascot,
                    grid,
                    Pose {
                        blink: 0.,
                        ..at(mood, 100)
                    },
                )
            };
            assert!(count(&open(Mood::Working), Cell::Eye) > count(&open(Mood::Awake), Cell::Eye));
            let lifts: Vec<f32> = (0..3000)
                .step_by(40)
                .map(|ms| at(Mood::Working, ms).lift)
                .collect();
            assert!(
                lifts.iter().any(|l| *l < -0.03),
                "{} doesn't bounce",
                mascot.id
            );
            // Delegating: eyes sweep both ways.
            let sweeps: Vec<f32> = (0..6000)
                .step_by(50)
                .map(|ms| at(Mood::Delegating, ms).gaze.0)
                .collect();
            assert!(sweeps.iter().any(|g| *g > 0.06) && sweeps.iter().any(|g| *g < -0.06));
            // Needs you: hops every couple of seconds, with eyes open.
            let hops = (0..2200)
                .step_by(20)
                .filter(|ms| at(Mood::NeedsYou, *ms).lift < -0.06)
                .count();
            assert!(hops > 0, "{} never hops", mascot.id);
            assert!(at(Mood::NeedsYou, 0).wide > at(Mood::Working, 0).wide);
            // Without motion, each mood still looks different.
            let still: std::collections::HashSet<_> = [
                Mood::Awake,
                Mood::Resting,
                Mood::Working,
                Mood::Delegating,
                Mood::NeedsYou,
            ]
            .into_iter()
            .map(|mood| raster(mascot, grid, still_pose(mood)))
            .collect();
            assert_eq!(still.len(), 5, "{}", mascot.id);
        }
    }

    #[test]
    fn configs_saved_before_custom_images_still_load() {
        use crate::zots::{ZotConfig, avatar_image_ok};
        let old = r#"{"name":"Ops","purpose":"","homeDeviceId":"d","repositories":[],
            "coordinatorModel":{"harness":"claude-code","model":null},"workerDeviceId":"d",
            "maxConcurrentWorkers":4,"maxFollowups":3,"avatar":{"mascot":"hex","color":"pink"}}"#;
        let config: ZotConfig = serde_json::from_str(old).unwrap();
        assert_eq!(config.avatar_image, None);
        assert_eq!(resolve(config.avatar.as_ref().unwrap()).0.name, "Orbit");
        // Without an image the saved shape is unchanged, so older clients
        // and engines read what newer ones write.
        let saved = serde_json::to_value(&config).unwrap();
        assert!(saved.get("avatarImage").is_none());
        assert_eq!(
            saved["avatar"],
            serde_json::json!({"mascot":"hex","color":"pink"})
        );
        let image = "data:image/png;base64,iVBORw0KGgo=";
        let with = ZotConfig {
            avatar_image: Some(image.into()),
            ..config
        };
        let text = serde_json::to_string(&with).unwrap();
        assert!(text.contains(r#""avatarImage":"data:image/png;base64,iVBORw0KGgo=""#));
        assert_eq!(serde_json::from_str::<ZotConfig>(&text).unwrap(), with);
        // The avatar itself never gains fields, so personas stay small.
        let avatar: ZotAvatar =
            serde_json::from_str(r#"{"mascot":"bolt","color":"teal","future":1}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&avatar).unwrap(),
            r#"{"mascot":"bolt","color":"teal"}"#
        );

        assert!(avatar_image_ok(image));
        assert!(avatar_image_ok("data:image/jpeg;base64,/9j/4AAQ"));
        for bad in [
            "",
            "data:image/png;base64,",
            "data:image/svg+xml;base64,PHN2Zz4=",
            "https://example.com/a.png",
            "data:image/png;base64,abc\"<x>",
        ] {
            assert!(!avatar_image_ok(bad), "{bad}");
        }
        let huge = format!(
            "data:image/png;base64,{}",
            "A".repeat(crate::zots::AVATAR_IMAGE_MAX_LEN)
        );
        assert!(!avatar_image_ok(&huge));
    }
}
