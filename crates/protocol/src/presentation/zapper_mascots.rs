//! Zapper avatars, shared by every client: soft flat mascots that turn
//! toward you.
//!
//! Each mascot is a [`Body`]: a flat-colour blob (a superellipse, optionally
//! tapered into a drop) with big capsule or oval eyes. A [`Pose`] turns it in
//! fake 3D: the eyes ride the curved front of the body, sliding and
//! foreshortening as it yaws and pitches, while roll, lift and squash move
//! the silhouette, and lids, worry and smile change the eyes' shape.
//! [`outline`] turns a posed mascot into smooth filled outlines that every
//! renderer draws the same way. [`idle_pose`] supplies the life: irregular
//! blinks, glances, a head that follows a look target (the pointer), and a
//! [`Mood`] that shows what the Zapper is doing, from closed eyes at rest
//! to a focused bounce at work and happy arcs when it is done. An
//! [`Animator`] blends one mood into the next. Mascot and colour ids are saved with
//! each Zapper, so art and names can change but ids cannot; every id saved
//! by an earlier version still names one of these.
use crate::zappers::ZapperAvatar;

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

/// A stable avatar for a seed, so Zappers saved without one keep their look.
pub fn default_avatar(seed: &str) -> ZapperAvatar {
    let value = hash(seed);
    ZapperAvatar {
        mascot: MASCOTS[value as usize % MASCOTS.len()].id.into(),
        color: COLORS[(value >> 8) as usize % COLORS.len()].id.into(),
    }
}

/// The mascot and colour an avatar draws, unknown ids falling back to the
/// first of each so a Zapper saved by a newer client still has a face.
pub fn resolve(avatar: &ZapperAvatar) -> (&'static Mascot, &'static MascotColor) {
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

/// Where a mascot is looking, how it is held and what its eyes say, in
/// degrees and avatar units.
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
    /// Upper lids lowered into a focused look: 0 is open, 1 covers half the eye.
    pub lid: f32,
    /// Lids slanting down toward the outer corners, a worried look: 0 is
    /// level, 1 is a clear droop.
    pub worry: f32,
    /// Eyes curved into happy arcs: 0 is round, 1 is an arc.
    pub smile: f32,
}

impl Pose {
    /// The pose `t` of the way from `self` to `to`.
    pub fn mix(self, to: Pose, t: f32) -> Pose {
        let m = |a: f32, b: f32| a + (b - a) * t;
        Pose {
            yaw: m(self.yaw, to.yaw),
            pitch: m(self.pitch, to.pitch),
            roll: m(self.roll, to.roll),
            lift: m(self.lift, to.lift),
            squash: m(self.squash, to.squash),
            gaze: (m(self.gaze.0, to.gaze.0), m(self.gaze.1, to.gaze.1)),
            blink: m(self.blink, to.blink),
            wide: m(self.wide, to.wide),
            lid: m(self.lid, to.lid),
            worry: m(self.worry, to.worry),
            smile: m(self.smile, to.smile),
        }
    }
}

/// What a Zapper is doing, as its mascot shows it. Each mood comes from
/// state the client already has: `mood_of` in the shell and `moodOf` in
/// the web client hold the mapping.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Mood {
    /// Active with nothing running, or nothing known: calm and a little
    /// curious, glancing around now and then.
    #[default]
    Awake,
    /// Paused or stopped: eyes closed, head dipped, still.
    Resting,
    /// Its home device is offline: eyes closed and the picture dimmed, so
    /// stale state never reads as activity.
    Offline,
    /// The Zapper itself is running a turn, thinking or using tools: focused
    /// eyes and a steady bounce.
    Working,
    /// Only its threads are working: eyes scanning back and forth.
    Delegating,
    /// Waiting on you: wide eyes on the pointer and a hop now and then.
    NeedsYou,
    /// Its last turn failed: worried eyes and a small shake now and then.
    Blocked,
    /// Finished, with the result not seen yet: happy arcs for eyes.
    Done,
}

impl Mood {
    /// Words for the mood, for tooltips and screen readers.
    pub fn label(self) -> &'static str {
        match self {
            Mood::Awake => "Idle",
            Mood::Resting => "Resting",
            Mood::Offline => "Offline",
            Mood::Working => "Working",
            Mood::Delegating => "Threads working",
            Mood::NeedsYou => "Needs you",
            Mood::Blocked => "Needs attention",
            Mood::Done => "Done",
        }
    }

    /// Whether the mood moves all the time rather than in short moments.
    pub fn busy(self) -> bool {
        matches!(self, Mood::Working | Mood::Delegating)
    }
}

/// The largest turn toward a look target.
pub const LOOK_YAW: f32 = 12.;
pub const LOOK_PITCH: f32 = 8.;
/// How quickly eyes and head catch up with a look target, in ms.
pub const EYES_FOLLOW_MS: f32 = 72.;
pub const HEAD_FOLLOW_MS: f32 = 190.;
/// How long a change of mood takes to blend, in ms.
pub const TRANSITION_MS: u64 = 420;
/// How long the gesture that greets a new mood lasts, in ms.
pub const GREETING_MS: u64 = 620;

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

/// Where the eyes rest at `ms` when they move between fixed points: every
/// `period` they travel to the next point in `travel` ms, then hold, so
/// between glances nothing moves.
fn glance(mascot: &Mascot, ms: u64, period: u64, travel: f32, points: &[(f32, f32)]) -> (f32, f32) {
    let t = ms + u64::from(hash(mascot.id)) % period;
    let step = (t / period) as usize;
    let k = ease_gentle(((t % period) as f32 / travel).min(1.));
    let (from, to) = (
        points[(step + points.len() - 1) % points.len()],
        points[step % points.len()],
    );
    (from.0 + (to.0 - from.0) * k, from.1 + (to.1 - from.1) * k)
}

/// A brief gesture repeating every `period` ms: how far through it the
/// mascot is at `ms`, or `None` between gestures.
fn now_and_then(mascot: &Mascot, ms: u64, period: u64, length: u64) -> Option<f32> {
    let t = (ms + u64::from(hash(mascot.id)) % period) % period;
    (t < length).then(|| t as f32 / length as f32)
}

/// The pose at `ms` in `mood`, turned by `eyes` and `head`: smoothed look
/// targets in -1..1, positive toward the right and down. Only the busy moods
/// move all the time; the others hold still between blinks, glances and
/// gestures, so a quiet roster costs nothing to draw.
pub fn idle_pose(mascot: &Mascot, mood: Mood, ms: u64, eyes: (f32, f32), head: (f32, f32)) -> Pose {
    let s = (ms as f32 / 1000.) + (hash(mascot.id) % 1000) as f32 / 37.;
    let sway = |rate: f32, phase: f32| (s * rate + phase).sin();
    let looking = Pose {
        yaw: head.0 * LOOK_YAW,
        pitch: head.1 * LOOK_PITCH,
        roll: head.0 * head.1 * 2.2,
        gaze: (eyes.0 * 0.08, eyes.1 * 0.06),
        blink: blink_at(mascot, ms),
        ..Pose::default()
    };
    match mood {
        Mood::Awake => {
            // Mostly looking ahead, now and then a curious glance.
            const AROUND: [(f32, f32); 6] = [
                (0., 0.),
                (0.8, -0.2),
                (0., 0.),
                (-0.7, 0.3),
                (0., 0.),
                (0.3, 0.6),
            ];
            let g = glance(mascot, ms, 3100, 260., &AROUND);
            Pose {
                yaw: looking.yaw + g.0 * 4.,
                pitch: looking.pitch + g.1 * 3.,
                gaze: (looking.gaze.0 + g.0 * 0.08, looking.gaze.1 + g.1 * 0.05),
                ..looking
            }
        }
        Mood::Resting => Pose {
            pitch: 5.,
            blink: 1.,
            ..Pose::default()
        },
        Mood::Offline => Pose {
            pitch: 2.,
            blink: 1.,
            ..Pose::default()
        },
        Mood::Working => {
            // Focused: lids lowered, eyes moving between points of work, a
            // steady light bounce.
            const WORK: [(f32, f32); 4] = [(0., 0.3), (0.6, 0.35), (-0.5, 0.3), (0.1, 0.)];
            let g = glance(mascot, ms, 1500, 170., &WORK);
            let bounce = sway(3.8, 0.).abs();
            Pose {
                yaw: g.0 * 5. + head.0 * LOOK_YAW * 0.5,
                pitch: g.1 * 3. + head.1 * LOOK_PITCH * 0.5,
                lift: -bounce * 0.07,
                squash: (1. - bounce) * 0.05 - 0.015,
                gaze: (g.0 * 0.08 + eyes.0 * 0.04, g.1 * 0.05 + eyes.1 * 0.03),
                wide: 0.04,
                lid: 0.38,
                ..looking
            }
        }
        Mood::Delegating => {
            // Watching its threads: eyes sweep side to side, the head trails.
            let sweep = sway(1.9, 0.);
            Pose {
                yaw: sway(1.9, -0.6) * 5. + head.0 * LOOK_YAW * 0.4,
                gaze: (sweep * 0.11 + eyes.0 * 0.03, looking.gaze.1),
                lid: 0.2,
                ..looking
            }
        }
        Mood::NeedsYou => {
            // Calling for you: wide eyes on the pointer, a hop now and then.
            let mut pose = Pose {
                pitch: looking.pitch - 3.,
                gaze: (eyes.0 * 0.11, eyes.1 * 0.08),
                wide: 0.3,
                ..looking
            };
            if let Some(p) = now_and_then(mascot, ms, 2400, 380) {
                let hop = (p * std::f32::consts::PI).sin();
                pose.lift = -hop * 0.12;
                pose.squash = -hop * 0.05;
                pose.blink = 0.;
            }
            pose
        }
        Mood::Blocked => {
            // Worried: lids slanting down, eyes low, a small shake now and then.
            let mut pose = Pose {
                pitch: looking.pitch + 4.,
                gaze: (looking.gaze.0, looking.gaze.1 + 0.04),
                lid: 0.1,
                worry: 1.,
                ..looking
            };
            if let Some(p) = now_and_then(mascot, ms, 3400, 520) {
                pose.yaw += (p * std::f32::consts::TAU * 2.).sin() * 6. * (1. - p);
            }
            pose
        }
        Mood::Done => Pose {
            pitch: looking.pitch - 2.,
            smile: 1.,
            blink: 0.,
            ..looking
        },
    }
}

/// A still pose that still says what the Zapper is doing, for reduced motion.
pub fn still_pose(mood: Mood) -> Pose {
    match mood {
        Mood::Awake => Pose::default(),
        Mood::Resting => Pose {
            pitch: 5.,
            blink: 1.,
            ..Pose::default()
        },
        Mood::Offline => Pose {
            pitch: 2.,
            blink: 1.,
            ..Pose::default()
        },
        Mood::Working => Pose {
            wide: 0.04,
            lid: 0.38,
            gaze: (0., 0.04),
            ..Pose::default()
        },
        Mood::Delegating => Pose {
            yaw: 4.,
            gaze: (0.1, 0.),
            lid: 0.2,
            ..Pose::default()
        },
        Mood::NeedsYou => Pose {
            wide: 0.3,
            pitch: -5.,
            ..Pose::default()
        },
        Mood::Blocked => Pose {
            pitch: 4.,
            gaze: (0., 0.04),
            lid: 0.1,
            worry: 1.,
            ..Pose::default()
        },
        Mood::Done => Pose {
            pitch: -2.,
            smile: 1.,
            ..Pose::default()
        },
    }
}

/// The gesture a mascot makes on entering `to` from `from`, `p` of the way
/// through [`GREETING_MS`], added to its pose.
fn greet(from: Mood, to: Mood, p: f32, pose: &mut Pose) {
    let arc = (p * std::f32::consts::PI).sin();
    match to {
        // Work arrived: a quick, wide-eyed hop before it settles to focus.
        Mood::Working | Mood::Delegating if !from.busy() => {
            pose.lift -= arc * 0.08;
            pose.squash -= arc * 0.05;
            pose.wide += arc * 0.25 * (1. - p);
            pose.lid *= p;
            pose.blink *= p;
        }
        // Finished: a small happy bounce.
        Mood::Done => {
            pose.lift -= arc * 0.06;
            pose.squash -= arc * 0.03;
        }
        // Calling: look up sharply.
        Mood::NeedsYou => {
            pose.lift -= arc * 0.07;
            pose.wide += arc * 0.1;
        }
        // Something went wrong: one shake.
        Mood::Blocked => {
            pose.yaw += (p * std::f32::consts::TAU * 2.).sin() * 7. * (1. - p);
        }
        _ => {}
    }
}

/// One mascot's mood over time: blends each change of mood from wherever the
/// mascot was, and greets the new mood with a gesture. The first mood shows
/// at once, so a mascot drawn for the first time never replays a change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Animator {
    mood: Mood,
    previous: Mood,
    from: Option<Pose>,
    changed: u64,
}

impl Animator {
    pub fn new(mood: Mood) -> Self {
        Self {
            mood,
            previous: mood,
            from: None,
            changed: 0,
        }
    }

    pub fn mood(&self) -> Mood {
        self.mood
    }

    /// Move to `mood` at `ms`, starting from the pose shown at that moment.
    pub fn set(
        &mut self,
        mascot: &Mascot,
        mood: Mood,
        ms: u64,
        eyes: (f32, f32),
        head: (f32, f32),
    ) {
        if mood == self.mood {
            return;
        }
        self.from = Some(self.pose(mascot, ms, eyes, head));
        self.previous = self.mood;
        self.mood = mood;
        self.changed = ms;
    }

    /// Whether a change of mood is still blending or greeting at `ms`.
    pub fn changing(&self, ms: u64) -> bool {
        self.from.is_some() && ms.saturating_sub(self.changed) < TRANSITION_MS.max(GREETING_MS)
    }

    /// The pose at `ms`.
    pub fn pose(&self, mascot: &Mascot, ms: u64, eyes: (f32, f32), head: (f32, f32)) -> Pose {
        let mut pose = idle_pose(mascot, self.mood, ms, eyes, head);
        let Some(from) = self.from else {
            return pose;
        };
        let since = ms.saturating_sub(self.changed);
        if since < TRANSITION_MS {
            pose = from.mix(pose, ease_gentle(since as f32 / TRANSITION_MS as f32));
        }
        if since < GREETING_MS {
            greet(
                self.previous,
                self.mood,
                since as f32 / GREETING_MS as f32,
                &mut pose,
            );
        }
        pose
    }
}

/// A posed mascot as filled outlines in tile units: the tile spans -1..1 on
/// both axes, `y` pointing down. The body is drawn first, then each eye.
#[derive(Debug, Clone, PartialEq)]
pub struct Outline {
    pub body: Vec<(f32, f32)>,
    pub eyes: Vec<Vec<(f32, f32)>>,
}

/// Mascots are modelled with room to sway, then drawn this much larger so
/// they fill their tile.
const ZOOM: f32 = 1.16;
/// Eyes are drawn this much larger than modelled, so they read at sidebar size.
const EYE_SCALE: f32 = 1.25;
/// Points around the body; enough for a smooth edge at 64px.
const BODY_POINTS: usize = 72;
/// Points along each edge of an eye.
const EYE_POINTS: usize = 13;

/// The thinnest an eye may get, in tile units, for an avatar `size` logical
/// pixels across: closed eyes stay a visible line at every size.
pub fn line_for(size: f32) -> f32 {
    (1.3 / (size / 2.) / ZOOM).clamp(0.03, 0.2)
}

/// `mascot` in `pose` as smooth outlines. `line` is the thinnest an eye may
/// get, from [`line_for`].
pub fn outline(mascot: &Mascot, pose: Pose, line: f32) -> Outline {
    let body = mascot.body;
    let (yaw, pitch, roll) = (
        pose.yaw.to_radians(),
        pose.pitch.to_radians(),
        pose.roll.to_radians(),
    );
    let centre_y = body.dy + pose.lift;
    let (sin, cos) = roll.sin_cos();
    // From body coordinates to the tile: squash, tilt, place, zoom.
    let place = |x: f32, y: f32| {
        let (x, y) = (x * (1. + pose.squash * 0.5), y * (1. - pose.squash));
        let (x, y) = (x * cos - y * sin, x * sin + y * cos);
        (x * ZOOM, (y + centre_y) * ZOOM)
    };
    // A turned body looks a touch narrower.
    let a = body.a * (1. - 0.08 * yaw.sin().powi(2));
    let power = |v: f32, n: f32| v.signum() * v.abs().powf(2. / n);
    let outline_body = (0..BODY_POINTS)
        .map(|i| {
            let t = i as f32 / BODY_POINTS as f32 * std::f32::consts::TAU;
            let y = body.b * power(t.sin(), body.n);
            let half = a * (1. - body.taper * (0.5 - 0.5 * (y / body.b)));
            place(half * power(t.cos(), body.n), y)
        })
        .collect();
    let depth = 0.9 * body.a.min(body.b);
    let mut eyes = Vec::with_capacity(body.eyes.len());
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
        let grow = EYE_SCALE * (1. + pose.wide);
        let w = eye.w * grow * squeeze(x1.atan2(z1), eye.x.atan2(z0));
        let h = eye.h * grow * squeeze(y2.atan2(z2), eye.y.atan2(z0));
        // Look far enough and the eye would slide off the face: pull it back
        // toward the middle until it sits inside the body with a margin.
        let (mut cx, mut cy) = (x1 + pose.gaze.0, y2 + pose.gaze.1);
        let fits = |cx: f32, cy: f32| {
            [
                (-1., -1.),
                (1., -1.),
                (-1., 1.),
                (1., 1.),
                (0., -1.),
                (0., 1.),
                (-1., 0.),
                (1., 0.),
            ]
            .iter()
            .all(|(sx, sy)| {
                let (x, y) = (cx + sx * w * 0.42, cy + sy * h * 0.42);
                let half = a * (1. - body.taper * (0.5 - 0.5 * (y / body.b).clamp(-1., 1.)));
                (x / half).abs().powf(body.n) + (y / body.b).abs().powf(body.n) <= 0.9
            })
        };
        if !fits(cx, cy) {
            let (mut lo, mut hi) = (0., 1.);
            for _ in 0..10 {
                let k = (lo + hi) / 2.;
                if fits(cx * k, cy * k) {
                    lo = k;
                } else {
                    hi = k;
                }
            }
            (cx, cy) = (cx * lo, cy * lo);
        }
        // Lids droop toward the outer corner: away from the face's middle.
        let outward = if eye.x < 0. { -1. } else { 1. };
        let (top, bottom) = eye_edges(eye.shape, w, h, pose, line, outward);
        eyes.push(
            top.iter()
                .chain(bottom.iter().rev())
                .map(|(x, y)| place(cx + x, cy + y))
                .collect(),
        );
    }
    Outline {
        body: outline_body,
        eyes,
    }
}

/// An eye's upper and lower edge, left to right, relative to its centre.
fn eye_edges(
    shape: EyeShape,
    w: f32,
    h: f32,
    pose: Pose,
    line: f32,
    outward: f32,
) -> (Vec<(f32, f32)>, Vec<(f32, f32)>) {
    let (rx, ry) = (w / 2., h / 2.);
    // Half the eye's height at `x`, open.
    let half = |x: f32| -> f32 {
        let x = x.abs().min(rx);
        match shape {
            EyeShape::Oval => ry * (1. - (x / rx).powi(2)).max(0.).sqrt(),
            EyeShape::Capsule if ry >= rx => (ry - rx) + (rx * rx - x * x).max(0.).sqrt(),
            EyeShape::Capsule => {
                let straight = rx - ry;
                if x <= straight {
                    ry
                } else {
                    (ry * ry - (x - straight).powi(2)).max(0.).sqrt()
                }
            }
        }
    };
    let blink = pose.blink.clamp(0., 1.);
    let open = 1. - blink;
    let smile = pose.smile.clamp(0., 1.);
    // A happy eye is an arch at least as wide as the eye is tall.
    let wide = rx.max(ry * 0.8);
    let arch = wide * 0.85;
    let band = (h * 0.24).max(line * 1.5);
    let mut top = Vec::with_capacity(EYE_POINTS);
    let mut bottom = Vec::with_capacity(EYE_POINTS);
    for i in 0..EYE_POINTS {
        // Spaced by cosine, so the rounded ends get the most points.
        let u = -(std::f32::consts::PI * i as f32 / (EYE_POINTS - 1) as f32).cos();
        let x = u * rx;
        let across = (1. - u * u).max(0.).sqrt();
        let thin = line * 0.5 * across.sqrt();
        let open_half = half(x) * open;
        // Closed eyes come to rest a little low, as lids do.
        let drop = ry * 0.25 * blink;
        let reach = open_half.max(thin);
        let (mut t, mut b) = (drop - reach, drop + reach);
        // Lids: lowered straight across, and slanting down toward the
        // outer corner when worried.
        let lid = ry * open * (-1. + pose.lid + pose.worry * (0.1 + 0.4 * outward * u));
        t = t.max(lid).min(b - 2. * thin);
        let mut x = x;
        // Happy arcs replace the round eye.
        if smile > 0. {
            let arc_top = arch * (0.4 - across);
            let arc_bottom = arc_top + band * (0.5 + 0.5 * across.sqrt());
            x += (u * wide - x) * smile;
            t += (arc_top - t) * smile;
            b += (arc_bottom - b) * smile;
        }
        top.push((x, t));
        bottom.push((x, b));
    }
    (top, bottom)
}

/// The area of a closed polygon.
pub fn area(points: &[(f32, f32)]) -> f32 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum::<f32>()
        .abs()
        / 2.
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
        assert_eq!(default_avatar("zapper"), default_avatar("zapper"));
        let unknown = ZapperAvatar {
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
            let avatar = ZapperAvatar {
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

    const LINE: f32 = 0.08;

    /// Mean of a set of points.
    fn centre(points: &[(f32, f32)]) -> (f32, f32) {
        let n = points.len() as f32;
        (
            points.iter().map(|p| p.0).sum::<f32>() / n,
            points.iter().map(|p| p.1).sum::<f32>() / n,
        )
    }

    fn eyes_at(shape: &Outline) -> (f32, f32) {
        let all: Vec<_> = shape.eyes.iter().flatten().copied().collect();
        centre(&all)
    }

    fn eye_area(shape: &Outline) -> f32 {
        shape.eyes.iter().map(|eye| area(eye)).sum()
    }

    /// Tallest eye, top to bottom.
    fn eye_height(shape: &Outline) -> f32 {
        shape
            .eyes
            .iter()
            .map(|eye| {
                let (lo, hi) = eye.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
                    (lo.min(p.1), hi.max(p.1))
                });
                hi - lo
            })
            .fold(0., f32::max)
    }

    fn inside(polygon: &[(f32, f32)], (x, y): (f32, f32)) -> bool {
        let mut hit = false;
        let n = polygon.len();
        for i in 0..n {
            let (a, b) = (polygon[i], polygon[(i + n - 1) % n]);
            if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1) + a.0 {
                hit = !hit;
            }
        }
        hit
    }

    #[test]
    fn mascots_fill_the_tile_show_their_eyes_and_differ() {
        let mut seen = Vec::<Outline>::new();
        for mascot in MASCOTS {
            assert!(!mascot.about.is_empty());
            let shape = outline(mascot, Pose::default(), LINE);
            assert!(
                !seen.iter().any(|other| *other == shape),
                "{} repeats",
                mascot.id
            );
            // The tile is 2 x 2 units.
            assert!(area(&shape.body) > 4. * 0.35, "{} too small", mascot.id);
            assert_eq!(shape.eyes.len(), 2, "{} lost an eye", mascot.id);
            for eye in &shape.eyes {
                assert!(area(eye) > 0.01, "{} has a dot for an eye", mascot.id);
            }
            for point in shape.body.iter().chain(shape.eyes.iter().flatten()) {
                assert!(
                    point.0.abs() <= 1. && point.1.abs() <= 1.,
                    "{} leaves its tile",
                    mascot.id
                );
            }
            // Symmetric faces come out symmetric at rest.
            if mascot.id != "rogue" {
                let (left, right) = (centre(&shape.eyes[0]), centre(&shape.eyes[1]));
                assert!(
                    (left.0 + right.0).abs() < 1e-3 && (left.1 - right.1).abs() < 1e-3,
                    "{} is lopsided",
                    mascot.id
                );
                assert!(
                    (area(&shape.eyes[0]) - area(&shape.eyes[1])).abs() < 1e-4,
                    "{}",
                    mascot.id
                );
                assert!(centre(&shape.body).0.abs() < 1e-3, "{}", mascot.id);
            }
            seen.push(shape);
        }
    }

    #[test]
    fn turning_moves_the_eyes_across_the_face() {
        for mascot in MASCOTS {
            let rest = eyes_at(&outline(mascot, Pose::default(), LINE));
            let right = Pose {
                yaw: LOOK_YAW,
                ..Pose::default()
            };
            let down = Pose {
                pitch: LOOK_PITCH,
                ..Pose::default()
            };
            assert!(
                eyes_at(&outline(mascot, right, LINE)).0 > rest.0,
                "{}",
                mascot.id
            );
            assert!(
                eyes_at(&outline(mascot, down, LINE)).1 > rest.1,
                "{}",
                mascot.id
            );
            // Eyes never leave the body, even at the widest turn, tilt and
            // the most expressive faces.
            for pose in [
                Pose {
                    yaw: -LOOK_YAW - 6.,
                    pitch: -LOOK_PITCH - 5.,
                    roll: 4.,
                    lift: 0.035,
                    gaze: (-0.11, -0.08),
                    wide: 0.4,
                    ..Pose::default()
                },
                Pose {
                    yaw: LOOK_YAW + 6.,
                    pitch: LOOK_PITCH + 4.,
                    gaze: (0.11, 0.08),
                    worry: 1.,
                    ..Pose::default()
                },
                Pose {
                    smile: 1.,
                    lift: -0.09,
                    squash: -0.05,
                    ..Pose::default()
                },
            ] {
                let shape = outline(mascot, pose, LINE);
                assert!(!shape.eyes.is_empty(), "{}", mascot.id);
                for point in shape.eyes.iter().flatten() {
                    assert!(
                        inside(&shape.body, *point),
                        "{} eye leaves the body at {point:?} in {pose:?}",
                        mascot.id
                    );
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
            let open = outline(mascot, Pose::default(), LINE);
            let closed = outline(
                mascot,
                Pose {
                    blink: 1.,
                    ..Pose::default()
                },
                LINE,
            );
            assert!(eye_area(&closed) < eye_area(&open), "{}", mascot.id);
            // A closed eye is still a visible line.
            assert!(eye_height(&closed) >= LINE * 0.99, "{}", mascot.id);
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
            assert!(pose.yaw.abs() <= 4. && pose.pitch.abs() <= 3. && pose.roll.abs() <= 1.6);
            assert!(pose.lift.abs() <= 0.035 && pose.squash.abs() <= 0.02);
        }
        let looking = idle_pose(mascot, Mood::Awake, 0, (1., 1.), (1., 1.));
        let ahead = idle_pose(mascot, Mood::Awake, 0, (0., 0.), (0., 0.));
        assert!(looking.yaw - ahead.yaw > LOOK_YAW - 0.01);
        assert!(looking.pitch - ahead.pitch > LOOK_PITCH - 0.01);
        assert!(looking.gaze.0 > ahead.gaze.0 && looking.gaze.1 > ahead.gaze.1);
    }

    const MOODS: [Mood; 8] = [
        Mood::Awake,
        Mood::Resting,
        Mood::Offline,
        Mood::Working,
        Mood::Delegating,
        Mood::NeedsYou,
        Mood::Blocked,
        Mood::Done,
    ];

    #[test]
    fn moods_read_at_a_glance() {
        for mascot in MASCOTS {
            let at = |mood: Mood, ms: u64| idle_pose(mascot, mood, ms, (0., 0.), (0., 0.));
            let shape = |pose: Pose| outline(mascot, pose, LINE);
            let awake = shape(Pose {
                blink: 0.,
                ..at(Mood::Awake, 100)
            });
            // Resting and offline: eyes are closed lines the whole time.
            for mood in [Mood::Resting, Mood::Offline] {
                for ms in (0..20_000).step_by(250) {
                    let closed = shape(at(mood, ms));
                    assert!(
                        eye_height(&closed) <= (eye_height(&awake) * 0.45).max(LINE * ZOOM * 1.01),
                        "{} opens its eyes",
                        mascot.id
                    );
                    assert_eq!(closed.eyes.len(), 2, "{} loses its eyes", mascot.id);
                }
            }
            // Working: focused lids, and it bounces.
            let open = |mood| {
                shape(Pose {
                    blink: 0.,
                    ..at(mood, 100)
                })
            };
            assert!(
                eye_height(&open(Mood::Working)) < eye_height(&awake),
                "{}",
                mascot.id
            );
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
            // Needs you: hops every couple of seconds, with eyes open and wide.
            let hops = (0..2400)
                .step_by(20)
                .filter(|ms| at(Mood::NeedsYou, *ms).lift < -0.06)
                .count();
            assert!(hops > 0, "{} never hops", mascot.id);
            assert!(
                eye_area(&open(Mood::NeedsYou)) > eye_area(&awake),
                "{}",
                mascot.id
            );
            // Done: happy arcs, so the eyes hold less ink than round ones.
            assert!(
                eye_area(&shape(at(Mood::Done, 100))) < eye_area(&awake),
                "{}",
                mascot.id
            );
            // Blocked: lids slant down toward the outer corners.
            let worried = shape(Pose {
                blink: 0.,
                ..still_pose(Mood::Blocked)
            });
            let left = &worried.eyes[0];
            let (inner, outer) = (left[EYE_POINTS - 3].1, left[2].1);
            assert!(outer > inner, "{} isn't worried", mascot.id);
            // Without motion, each mood still looks different.
            let still: Vec<Outline> = MOODS.iter().map(|mood| shape(still_pose(*mood))).collect();
            for (i, a) in still.iter().enumerate() {
                for b in &still[i + 1..] {
                    assert!(a != b, "{} has two moods that look alike", mascot.id);
                }
            }
        }
    }

    /// How many of `frames` samples, `step` ms apart, would draw differently
    /// from the one before, at quarter-pixel precision on a 32px avatar.
    fn changes(mascot: &Mascot, mood: Mood, frames: u64, step: u64) -> usize {
        let key = |ms: u64| {
            let shape = outline(
                mascot,
                idle_pose(mascot, mood, ms, (0., 0.), (0., 0.)),
                LINE,
            );
            shape
                .body
                .iter()
                .chain(shape.eyes.iter().flatten())
                .map(|p| ((p.0 * 64.).round() as i32, (p.1 * 64.).round() as i32))
                .collect::<Vec<_>>()
        };
        (1..frames)
            .filter(|i| key(i * step) != key((i - 1) * step))
            .count()
    }

    #[test]
    fn quiet_moods_hold_still_between_moments() {
        // Ten seconds at 30 frames a second.
        for mascot in MASCOTS {
            for mood in [Mood::Resting, Mood::Offline, Mood::Done] {
                assert_eq!(
                    changes(mascot, mood, 300, 33),
                    0,
                    "{} {mood:?} moves",
                    mascot.id
                );
            }
            for mood in [Mood::Awake, Mood::NeedsYou, Mood::Blocked] {
                let moving = changes(mascot, mood, 300, 33);
                assert!(
                    moving < 300 / 3,
                    "{} {mood:?} redraws {moving} of 300 frames",
                    mascot.id
                );
            }
            assert!(
                changes(mascot, Mood::Working, 300, 33) > 200,
                "{} works without moving",
                mascot.id
            );
        }
    }

    #[test]
    fn mood_changes_blend_from_where_the_mascot_was() {
        let mascot = &MASCOTS[0];
        let mut animator = Animator::new(Mood::Awake);
        // The first mood shows at once.
        assert_eq!(
            animator.pose(mascot, 5_000, (0., 0.), (0., 0.)),
            idle_pose(mascot, Mood::Awake, 5_000, (0., 0.), (0., 0.))
        );
        assert!(!animator.changing(5_000));
        let before = animator.pose(mascot, 10_000, (0., 0.), (0., 0.));
        animator.set(mascot, Mood::Working, 10_000, (0., 0.), (0., 0.));
        // Setting the same mood again changes nothing, so re-renders never
        // restart a transition.
        let again = animator;
        animator.set(mascot, Mood::Working, 10_100, (0., 0.), (0., 0.));
        assert_eq!(animator, again);
        // No jump at the moment of change, whatever the frame rate.
        let start = animator.pose(mascot, 10_000, (0., 0.), (0., 0.));
        assert!((start.lid - before.lid).abs() < 1e-4 && (start.lift - before.lift).abs() < 1e-4);
        for step in [8, 16, 33, 50] {
            let mut last = start;
            for ms in (10_000..10_000 + GREETING_MS + 100).step_by(step) {
                let pose = animator.pose(mascot, ms, (0., 0.), (0., 0.));
                assert!((pose.lid - last.lid).abs() <= 0.3 * step as f32 / 50. + 1e-3);
                last = pose;
            }
        }
        // Work arriving is acknowledged with a hop, then it settles to focus.
        let hop = (10_000..10_000 + GREETING_MS)
            .step_by(10)
            .map(|ms| animator.pose(mascot, ms, (0., 0.), (0., 0.)).lift)
            .fold(0., f32::min);
        assert!(hop < -0.07);
        assert!(animator.changing(10_300) && !animator.changing(10_000 + GREETING_MS));
        let settled = animator.pose(mascot, 12_000, (0., 0.), (0., 0.));
        assert_eq!(
            settled,
            idle_pose(mascot, Mood::Working, 12_000, (0., 0.), (0., 0.))
        );
        // Finishing settles into happy eyes.
        animator.set(mascot, Mood::Done, 13_000, (0., 0.), (0., 0.));
        assert!(
            animator
                .pose(mascot, 13_000 + TRANSITION_MS, (0., 0.), (0., 0.))
                .smile
                > 0.99
        );
    }

    fn mood_name(mood: Mood) -> &'static str {
        match mood {
            Mood::Awake => "awake",
            Mood::Resting => "resting",
            Mood::Offline => "offline",
            Mood::Working => "working",
            Mood::Delegating => "delegating",
            Mood::NeedsYou => "needsYou",
            Mood::Blocked => "blocked",
            Mood::Done => "done",
        }
    }

    /// The web client ports this file; both check themselves against the same
    /// samples. `ZAPPER_PARITY_WRITE=1` rewrites them after a deliberate change.
    #[test]
    fn web_port_draws_the_same_mascots() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../apps/web-solid/src/features/zappers/mascot-parity.json"
        );
        let round = |v: f32| (v * 1_000.).round() / 1_000.;
        let points = |list: &[(f32, f32)]| {
            list.iter()
                .map(|p| [round(p.0), round(p.1)])
                .collect::<Vec<_>>()
        };
        let mut samples = Vec::new();
        // Every mascot once, and every mood on the first.
        let cases = MASCOTS
            .iter()
            .enumerate()
            .map(|(i, _)| (i, i % MOODS.len()))
            .chain((1..MOODS.len()).map(|j| (0, j)));
        for (i, j) in cases {
            let (mascot, mood) = (&MASCOTS[i], &MOODS[j]);
            {
                let ms = 1_000 + (i * 977 + j * 1_313) as u64;
                let look = (0.3 - 0.1 * i as f32, -0.2 + 0.05 * j as f32);
                let pose = idle_pose(mascot, *mood, ms, look, look);
                let line = line_for(16. + 6. * j as f32);
                let shape = outline(mascot, pose, line);
                let numbers = [
                    pose.yaw,
                    pose.pitch,
                    pose.roll,
                    pose.lift,
                    pose.squash,
                    pose.gaze.0,
                    pose.gaze.1,
                    pose.blink,
                    pose.wide,
                    pose.lid,
                    pose.worry,
                    pose.smile,
                ]
                .map(round);
                samples.push(serde_json::json!({
                    "mascot": mascot.id, "mood": mood_name(*mood), "ms": ms, "look": [look.0, look.1], "line": line,
                    "pose": numbers,
                    "body": points(&shape.body.iter().step_by(4).copied().collect::<Vec<_>>()),
                    "eyes": shape.eyes.iter().map(|eye| points(&eye.iter().step_by(2).copied().collect::<Vec<_>>())).collect::<Vec<_>>(),
                }));
            }
        }
        let fresh = serde_json::Value::Array(samples);
        if std::env::var_os("ZAPPER_PARITY_WRITE").is_some() {
            std::fs::write(path, serde_json::to_string(&fresh).unwrap() + "\n").unwrap();
        }
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (saved, fresh) = (saved.as_array().unwrap(), fresh.as_array().unwrap());
        assert_eq!(saved.len(), fresh.len());
        let flat = |v: &serde_json::Value| -> Vec<f64> {
            let mut out = Vec::new();
            let mut stack = vec![v.clone()];
            while let Some(v) = stack.pop() {
                match v {
                    serde_json::Value::Number(n) => out.push(n.as_f64().unwrap()),
                    serde_json::Value::Array(list) => stack.extend(list),
                    _ => {}
                }
            }
            out
        };
        for (a, b) in saved.iter().zip(fresh) {
            for key in ["pose", "body", "eyes"] {
                let (x, y) = (flat(&a[key]), flat(&b[key]));
                assert_eq!(x.len(), y.len(), "{} {key}", a["mascot"]);
                assert!(
                    x.iter().zip(&y).all(|(x, y)| (x - y).abs() < 2e-3),
                    "{} {} {key} drifted",
                    a["mascot"],
                    a["mood"]
                );
            }
        }
    }

    #[test]
    fn eyes_stay_visible_at_sidebar_size() {
        let line = line_for(16.);
        // About 1.3 logical pixels at 16px.
        assert!((line * ZOOM * 8. - 1.3).abs() < 0.01);
        assert!(line_for(64.) < line && line_for(64.) >= 0.03);
        for mascot in MASCOTS {
            for mood in MOODS {
                let shape = outline(mascot, still_pose(mood), line);
                assert!(eye_height(&shape) * 8. >= 1.29, "{} {mood:?}", mascot.id);
            }
        }
    }

    #[test]
    fn configs_saved_before_custom_images_still_load() {
        use crate::zappers::{ZapperConfig, avatar_image_ok};
        let old = r#"{"name":"Ops","purpose":"","homeDeviceId":"d","repositories":[],
            "coordinatorModel":{"harness":"claude-code","model":null},"workerDeviceId":"d",
            "maxConcurrentWorkers":4,"maxFollowups":3,"avatar":{"mascot":"hex","color":"pink"}}"#;
        let config: ZapperConfig = serde_json::from_str(old).unwrap();
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
        let with = ZapperConfig {
            avatar_image: Some(image.into()),
            ..config
        };
        let text = serde_json::to_string(&with).unwrap();
        assert!(text.contains(r#""avatarImage":"data:image/png;base64,iVBORw0KGgo=""#));
        assert_eq!(serde_json::from_str::<ZapperConfig>(&text).unwrap(), with);
        // The avatar itself never gains fields, so personas stay small.
        let avatar: ZapperAvatar =
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
            "A".repeat(crate::zappers::AVATAR_IMAGE_MAX_LEN)
        );
        assert!(!avatar_image_ok(&huge));
    }
}
