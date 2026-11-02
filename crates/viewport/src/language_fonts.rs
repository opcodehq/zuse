//! Offline CJK coverage with regional UI glyphs and a full fallback for user text.
use gpui::{App, FontFallbacks};
use std::borrow::Cow;
use zuse_i18n::Locale;

const FACES: &[&[u8]] = &[
    include_bytes!("../assets/fonts/ZuseCJK-sc.otf"),
    include_bytes!("../assets/fonts/ZuseCJK-tc.otf"),
    include_bytes!("../assets/fonts/ZuseCJK-jp.otf"),
    include_bytes!("../assets/fonts/ZuseCJK-kr.otf"),
    include_bytes!("../assets/fonts/ZuseCJK-Fallback.ttf"),
    include_bytes!("../assets/fonts/ZuseKorean-Fallback.otf"),
];
pub fn faces() -> impl Iterator<Item = &'static [u8]> {
    FACES.iter().copied()
}
pub fn register(cx: &App) {
    cx.text_system()
        .add_fonts(faces().map(Cow::Borrowed).collect())
        .expect("bundled CJK fonts");
}
pub fn fallbacks(locale: Locale) -> FontFallbacks {
    let regional = match locale {
        Locale::ZhHant => "Zuse CJK TC",
        Locale::Ja => "Zuse CJK JP",
        Locale::Ko => "Zuse CJK KR",
        _ => "Zuse CJK SC",
    };
    FontFallbacks::from_fonts(vec![
        regional.into(),
        "Zuse Korean Fallback".into(),
        "Zuse CJK Fallback".into(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn bundled_regional_faces_cover_every_cjk_interface_character() {
        for (region, locale) in [(0, "zh-Hans"), (1, "zh-Hant"), (2, "ja"), (3, "ko")] {
            let face = ttf_parser::Face::parse(FACES[region], 0).unwrap();
            assert!(
                face.glyph_index('m').is_some(),
                "fallback faces need Latin metrics"
            );
            let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../i18n/locales")
                .join(locale);
            for file in std::fs::read_dir(directory).unwrap() {
                let text = std::fs::read_to_string(file.unwrap().path()).unwrap();
                for c in text
                    .chars()
                    .filter(|c| matches!(*c as u32,0x3000..=0x9fff|0xac00..=0xd7af))
                {
                    assert!(
                        face.glyph_index(c).is_some(),
                        "{locale}: missing UI glyph {c}; regenerate regional font subsets"
                    );
                }
            }
        }
    }
    #[test]
    fn full_fallback_supports_latin_metrics_and_user_cjk_text() {
        let face = ttf_parser::Face::parse(FACES[4], 0).unwrap();
        for c in "m你好繁體中文日本語".chars() {
            assert!(face.glyph_index(c).is_some(), "{c}");
        }
        let korean = ttf_parser::Face::parse(FACES[5], 0).unwrap();
        assert!(korean.glyph_index('m').is_some());
        for code in 0xAC00..=0xD7A3 {
            let c = char::from_u32(code).unwrap();
            assert!(korean.glyph_index(c).is_some(), "missing Hangul {c}");
        }
    }
}
