//! Pixel mad scientists for Zots, shared by every client.
//!
//! One character per pixel: `.` empty, `b` hair (the picked colour), `s` its shade,
//! `k` ink, `w` lab coat, `c` coat seam, `g` goggle glass, `t` potion, `p` skin.
//! Mascot ids are saved with each Zot, so art and names can change but ids cannot.
use crate::zots::ZotAvatar;

pub const SPRITE_SIZE: usize = 16;

pub struct Mascot {
    pub id: &'static str,
    pub name: &'static str,
    pub sprite: [&'static str; SPRITE_SIZE],
}

pub struct MascotColor {
    pub id: &'static str,
    pub body: u32,
    pub shade: u32,
}

pub const MASCOTS: &[Mascot] = &[
    Mascot {
        id: "bolt",
        name: "Dr. Volt",
        sprite: [
            "..b..b.bb.b..b..",
            ".b.bbbbbbbbbb.b.",
            "bbbbbbbbbbbbbbbb",
            ".bbbsssssssssbb.",
            ".bbppppppppppbb.",
            "..kkkkkkkkkkkk..",
            "..kggkppppkggk..",
            "..kkkkppppkkkk..",
            "...pppppppppp...",
            "...ppkwwwwkpp...",
            "....pppppppp....",
            ".....pppppp.....",
            "..wwwcwkkwcwww..",
            ".wwwwcwkkwcwwww.",
            ".wwwwcwwwwcwwww.",
            ".wwwwcwwwwcwwww.",
        ],
    },
    Mascot {
        id: "ace",
        name: "Dr. Flask",
        sprite: [
            "....bbbbbbbbb...",
            "..bbbbbbbbbbbbb.",
            ".bbbbbbbbbbsssbb",
            ".bbspppppppppsbb",
            "..bppppppppppb..",
            "..pkkkppppkkkp..",
            "..pkgkppppkgkp..",
            "..pkkkppppkkkp..",
            "..pppppppppppp..",
            "...pppkkkkppp...",
            "....pppppppp.t..",
            "...wwwwwwww.ttt.",
            "..wwwcwwcwwwtgt.",
            ".wwwwcwwcwwwttt.",
            ".wwwwcwwcwwww...",
            ".wwwwcwwcwwww...",
        ],
    },
    Mascot {
        id: "brawler",
        name: "Dr. Frizz",
        sprite: [
            "....bbbbbbbb....",
            "..bbbbbbbbbbbb..",
            ".bbbbbbbbbbbbbb.",
            "bbbbbbsbbsbbbbbb",
            "bbbbsssssssbbbbb",
            "bbbppppppppppbbb",
            "bbkkkkkppkkkkkbb",
            "bbkggkkkkkkggkbb",
            "bbkkkkppppkkkkbb",
            ".bbppppppppppbb.",
            "..bpppkkkkppb...",
            "....pppwwppp....",
            ".....pppppp.....",
            "..wwwcwkkwcwww..",
            ".wwwwcwkkwcwwww.",
            ".wwwwcwwwwcwwww.",
        ],
    },
    Mascot {
        id: "hex",
        name: "Dr. Hex",
        sprite: [
            "......kggk......",
            ".....kkkkkk.....",
            "....pppppppp....",
            "...pppppppppp...",
            "b.pppppppppppp.b",
            "bbppppppppppppbb",
            "bbpkkkppppkkkpbb",
            "bbpkgkkkkkkgkpbb",
            "bbpkkkppppkkkpbb",
            "b.pppppkkppppp.b",
            "...pppppppppp...",
            "...ppkkkkkkpp...",
            "....pppppppp....",
            "..wwwcwwwwcwww..",
            ".wwwwcwkkwcwwww.",
            ".wwwwcwwwwcwwww.",
        ],
    },
    Mascot {
        id: "squire",
        name: "Igor",
        sprite: [
            "...b...bb...b...",
            "..bbb.bbbb.bbb..",
            "..bbbbbbbbbbbb..",
            "..kkkkkkkkkkkk..",
            "..kggkbbbbkggk..",
            "..kkkkkkkkkkkk..",
            "..pppppppppppp..",
            "..ppkkppppkkpp..",
            "..ppwkppppwkpp..",
            "..pppppppppppp..",
            "..pkpkpkpkpkpp..",
            "...pppwwpppp....",
            "....pppppp......",
            "..wwwcwwwwcwww..",
            ".wwwwcwkkwcwwww.",
            ".wwwwcwwwwcwwww.",
        ],
    },
    Mascot {
        id: "shade",
        name: "Dr. Shade",
        sprite: [
            "................",
            ".....ssssss.....",
            "...ssbbbbbbss...",
            "..sbbbbbbbbbbs..",
            "..bbbbbbbbbbbb..",
            "..bkkkkkkkkkkb..",
            "..bkggggggggkb..",
            "..bkggggggggkb..",
            "..bkkkkkkkkkkb..",
            "..bbbbbbbbbbbb..",
            "..sbbkbkkbkbbs..",
            "...sbbbbbbbbs...",
            ".....ssssss.....",
            "..wwwcwkkwcwww..",
            ".wwwwcwkkwcwwww.",
            ".wwwwcwwwwcwwww.",
        ],
    },
    Mascot {
        id: "rogue",
        name: "Dr. Patch",
        sprite: [
            ".......bb.......",
            "......bbbb......",
            ".....bbbbbb.....",
            "......bbbb......",
            "...pppppppppp...",
            "..pppppppppppp..",
            "..kkkkkkkkkkkk..",
            "..kkkkkpppkggk..",
            "..kkkkkpppkkkk..",
            "..pkkppppppppp..",
            "...ppppppkkkpp..",
            "...pppppppppp...",
            "....pppppppp....",
            "..wwwcwwwwcwww..",
            ".wwwwcwkkwcwwww.",
            ".wwwwcwwwwcwwww.",
        ],
    },
    Mascot {
        id: "viking",
        name: "Dr. Whisk",
        sprite: [
            "..b.b.bbbb.b.b..",
            "..bbbbbbbbbbbb..",
            ".bbbbbbbbbbbbbb.",
            ".bbppppppppppbb.",
            "..pppppppppppp..",
            "..pkkkkkkkkkkp..",
            "..pkggkppkggkp..",
            "..pkkkkppkkkkp..",
            "..pppppppppppp..",
            ".bbbbbbppbbbbbb.",
            "bbbbbbbbbbbbbbbb",
            "bb.ppkkkkkkpp.bb",
            "....pppppppp....",
            "..wwwcwwwwcwww..",
            ".wwwwcwkkwcwwww.",
            ".wwwwcwwwwcwwww.",
        ],
    },
    Mascot {
        id: "bones",
        name: "Dr. Bones",
        sprite: [
            "..b.b.bbbb.b.b..",
            ".bbbbbbbbbbbbbb.",
            ".bbswwwwwwwwsbb.",
            "..wwwwwwwwwwww..",
            "..wwwwwwwwwwww..",
            "..kkkkkkkkkkkk..",
            "..kggkwwwwkggk..",
            "..kkkkwwwwkkkk..",
            "..wwwwwkkwwwww..",
            "...wwwwwwwwww...",
            "...wkwkwkwkww...",
            "....wkwkwkw.....",
            ".....ccccc......",
            "..wwwcwkkwcwww..",
            ".wwwwcwkkwcwwww.",
            ".wwwwcwwwwcwwww.",
        ],
    },
];

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

fn fixed_color(pixel: u8) -> Option<u32> {
    match pixel {
        b'k' => Some(0x1d1418),
        b'w' => Some(0xffffff),
        b'c' => Some(0xb9c3d1),
        b'g' => Some(0x7fe9ff),
        b't' => Some(0xff7aa8),
        b'p' => Some(0xf2c199),
        _ => None,
    }
}

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

/// Horizontal runs of `(x, y, width, rgb)` for one avatar, unknown ids falling
/// back to the first character and colour.
pub fn sprite_runs(avatar: &ZotAvatar) -> Vec<(u8, u8, u8, u32)> {
    let mascot = MASCOTS
        .iter()
        .find(|m| m.id == avatar.mascot)
        .unwrap_or(&MASCOTS[0]);
    let color = COLORS
        .iter()
        .find(|c| c.id == avatar.color)
        .unwrap_or(&COLORS[0]);
    let mut runs = Vec::new();
    for (y, row) in mascot.sprite.iter().enumerate() {
        let row = row.as_bytes();
        let mut x = 0;
        while x < row.len() {
            let pixel = row[x];
            let mut end = x + 1;
            while end < row.len() && row[end] == pixel {
                end += 1;
            }
            let rgb = match pixel {
                b'b' => Some(color.body),
                b's' => Some(color.shade),
                other => fixed_color(other),
            };
            if let Some(rgb) = rgb {
                runs.push((x as u8, y as u8, (end - x) as u8, rgb));
            }
            x = end;
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprites_are_square_and_only_use_known_pixels() {
        for mascot in MASCOTS {
            for row in mascot.sprite {
                assert_eq!(row.len(), SPRITE_SIZE, "{}", mascot.id);
                assert!(
                    row.bytes().all(|p| b".bskwcgtp".contains(&p)),
                    "{}",
                    mascot.id
                );
            }
        }
    }

    #[test]
    fn defaults_are_stable_and_unknown_ids_still_draw() {
        assert_eq!(default_avatar("zot"), default_avatar("zot"));
        let unknown = ZotAvatar {
            mascot: "gone".into(),
            color: "gone".into(),
        };
        assert!(!sprite_runs(&unknown).is_empty());
    }
}
