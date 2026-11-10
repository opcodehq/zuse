//! Pixel characters for Zots, shared by every client.
//!
//! One character per pixel: `.` empty, `b` body, `s` body shade, `k` ink, `w` white,
//! `h` horn, `f` fur, `t` tongue, `p` skin. Body colours come from the palette.
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
        name: "Bolt",
        sprite: [
            ".......hh.......",
            ".......hh.......",
            "...bbbbbbbbbb...",
            "..bbbbbbbbbbbs..",
            "..bkkkkkkkkkbs..",
            "..bkwwkkkkwwbs..",
            "..bkwwkkkkwwbs..",
            "..bkkkkkkkkkbs..",
            "..bbbbbbbbbbbs..",
            "h.bbkbkbkbkbbs.h",
            "hsbbbbbbbbbbbssh",
            "..ssssssssssss..",
            "...bbb....bbb...",
            "...bbs....bbs...",
            "..kkkk....kkkk..",
            "................",
        ],
    },
    Mascot {
        id: "ace",
        name: "Ace",
        sprite: [
            "....ssssssss....",
            "...bbbbbbbbbb...",
            "..bbbbbbbbbbbb..",
            "..bbbbbbbbbbss..",
            "..bkkkkbbkkkkb..",
            "..kwwwkkkkwwwk..",
            "..kwkwkbbkwkwk..",
            "..bkkkkbbkkkkb..",
            "..bppppppppppb..",
            "..bppppppppppb..",
            "..bppppppppppb..",
            "...ppkkkkkkpp...",
            "....pppppppp....",
            "...tttttttttt...",
            "..tttttttttttt..",
            "....tt....tt....",
        ],
    },
    Mascot {
        id: "brawler",
        name: "Brawler",
        sprite: [
            "...b..b..b..b...",
            "..bb.bb.bb.bb...",
            "..bbbbbbbbbbbb..",
            "..bbbbbbbbbbbb..",
            "..wwwwwwwwwwwwk.",
            "..wwwwwwwwwwwwkk",
            "..pppppppppppp..",
            "..pkkkppppkkkp..",
            "..pwkppppppwkp..",
            "..pppppppppppp..",
            "..ppppkppkpppp..",
            "..ppkkkkkkkkpp..",
            "...pppppppppp...",
            "..ssssssssssss..",
            ".pbbbbbbbbbbbbp.",
            ".pbbbbbbbbbbbbp.",
        ],
    },
    Mascot {
        id: "hex",
        name: "Hex",
        sprite: [
            ".......bb.......",
            "......bbbs......",
            ".....bbbbbs.....",
            "....bbwbbbbs....",
            "...bbbbbbwbbs...",
            "..bbbbbbbbbbss..",
            ".ssssssssssssss.",
            "...pppppppppp...",
            "...pwwkppkwwp...",
            "...pwkkppkkwp...",
            "...pppppppppp...",
            "....wwwwwwww....",
            "....wwwkkwww....",
            ".....wwwwww.....",
            "......wwww......",
            ".......ww.......",
        ],
    },
    Mascot {
        id: "squire",
        name: "Squire",
        sprite: [
            "......tt........",
            ".....ttttt......",
            "......ttttt.....",
            "....bbbbbbbb....",
            "...bbbbbbbbss...",
            "..bbbbbbbbbbss..",
            "..bbbbbbbbbbss..",
            "..skkkkkkkkkks..",
            "..kkwwkkkkwwkk..",
            "..skkkkkkkkkks..",
            "..bbbkbkkbkbbs..",
            "..bbbkbkkbkbbs..",
            "...bbbbbbbbss...",
            "....bsbsbsbs....",
            ".....ssssss.....",
            "................",
        ],
    },
    Mascot {
        id: "shade",
        name: "Shade",
        sprite: [
            "................",
            "....kkkkkkkk....",
            "...kkkkkkkkkk...",
            "..kkkkkkkkkkkkt.",
            "..kkkkkkkkkkkttt",
            "..kkbbbbbbbbkkt.",
            "..kbwwkbbkwwbkt.",
            "..kbwkkbbkkwbk..",
            "..kkbbbbbbbbkk..",
            "..kkkkkkkkkkkk..",
            "..kkkkkkkkkkkk..",
            "...kkkkkkkkkk...",
            "....kkkkkkkk....",
            "...ssskkkksss...",
            "...bbs....bbs...",
            "...kkk....kkk...",
        ],
    },
    Mascot {
        id: "rogue",
        name: "Rogue",
        sprite: [
            "................",
            "....bbbbbbbb....",
            "...bbwbbbbwbb...",
            "..bbbbbbbbbbbbs.",
            "..bbbbbbbbbbbsss",
            "..ssssssssssss.s",
            "..pppppppppppp..",
            "..pkkkkkppwwpp..",
            "..pkkkkkppwkpp..",
            "..pppppppppppp..",
            "..ppppphhppppp..",
            "..hhhhhhhhhhhh..",
            "..hhhkkkkkkhhh..",
            "...hhhhhhhhhh...",
            "....hhhhhhhh....",
            ".....hhhhhh.....",
        ],
    },
    Mascot {
        id: "viking",
        name: "Viking",
        sprite: [
            "w..............w",
            "ww............ww",
            "www..bbbbbb..www",
            ".wwbbbbbbbbbbww.",
            "..bbbbbbbbbbbb..",
            "..ssssssssssss..",
            "..pppppppppppp..",
            "..ppkkppppkkpp..",
            "..ppwkppppwkpp..",
            "..pppppppppppp..",
            "..ffffffffffff..",
            "..fffkkkkkkfff..",
            "..ffffffffffff..",
            "...ffffffffff...",
            "....ffffffff....",
            ".....ffffff.....",
        ],
    },
    Mascot {
        id: "bones",
        name: "Bones",
        sprite: [
            "....wwwwwwww....",
            "...wwwwwwwwww...",
            "..wwwwwwwwwwws..",
            "..wwwwwwwwwwws..",
            "..wkkkwwwkkkws..",
            "..wkbkwwwkbkws..",
            "..wkkkwwwkkkws..",
            "..wwwwwkwwwwws..",
            "...wwwkkkwwws...",
            "...wwwwwwwwws...",
            "....wkwkwkws....",
            "....wkwkwkws....",
            ".....ssssss.....",
            "................",
            "................",
            "................",
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
        b'h' => Some(0x9b4a2c),
        b'f' => Some(0xa0602f),
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
                    row.bytes().all(|p| b".bskwhftp".contains(&p)),
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
