//! Offline localization shared by the desktop, browser and CLI.
//! Each application owns its language context; immutable parsed bundles are cached.
use fluent_bundle::{FluentArgs, FluentResource, concurrent::FluentBundle};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

pub use fluent_bundle::{FluentArgs as Arguments, FluentValue};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Locale {
    #[default]
    #[serde(rename = "en")]
    En,
    #[serde(rename = "fr")]
    Fr,
    #[serde(rename = "de")]
    De,
    #[serde(rename = "zh-Hans")]
    ZhHans,
    #[serde(rename = "zh-Hant")]
    ZhHant,
    #[serde(rename = "ja")]
    Ja,
    #[serde(rename = "ko")]
    Ko,
}
impl Locale {
    pub const ALL: [Self; 7] = [
        Self::En,
        Self::Fr,
        Self::De,
        Self::ZhHans,
        Self::ZhHant,
        Self::Ja,
        Self::Ko,
    ];
    pub const fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Fr => "fr",
            Self::De => "de",
            Self::ZhHans => "zh-Hans",
            Self::ZhHant => "zh-Hant",
            Self::Ja => "ja",
            Self::Ko => "ko",
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Fr => "Français",
            Self::De => "Deutsch",
            Self::ZhHans => "简体中文",
            Self::ZhHant => "繁體中文",
            Self::Ja => "日本語",
            Self::Ko => "한국어",
        }
    }
    pub fn from_language(value: &str) -> Option<Self> {
        let normalized = value
            .split('.')
            .next()?
            .split('@')
            .next()?
            .replace('_', "-");
        let id: unic_langid::LanguageIdentifier = normalized.parse().ok()?;
        match id.language.as_str() {
            "en" => Some(Self::En),
            "fr" => Some(Self::Fr),
            "de" => Some(Self::De),
            "ja" => Some(Self::Ja),
            "ko" => Some(Self::Ko),
            "zh" => Some(match id.script.as_ref().map(|s| s.as_str()) {
                Some("Hant") => Self::ZhHant,
                Some("Hans") => Self::ZhHans,
                _ if id
                    .region
                    .as_ref()
                    .is_some_and(|r| matches!(r.as_str(), "TW" | "HK" | "MO")) =>
                {
                    Self::ZhHant
                }
                _ => Self::ZhHans,
            }),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(into = "String")]
pub enum LanguagePreference {
    #[default]
    System,
    Explicit(Locale),
}
impl<'de> Deserialize<'de> for LanguagePreference {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(value.as_str().unwrap_or("system").to_owned().into())
    }
}
impl From<String> for LanguagePreference {
    fn from(value: String) -> Self {
        Locale::ALL
            .into_iter()
            .find(|l| l.code() == value)
            .map(Self::Explicit)
            .unwrap_or_default()
    }
}
impl From<LanguagePreference> for String {
    fn from(value: LanguagePreference) -> String {
        match value {
            LanguagePreference::System => "system".into(),
            LanguagePreference::Explicit(l) => l.code().into(),
        }
    }
}
impl LanguagePreference {
    pub fn resolve(self, languages: &[String]) -> Locale {
        match self {
            Self::Explicit(locale) => locale,
            Self::System => languages
                .iter()
                .find_map(|s| Locale::from_language(s))
                .unwrap_or_default(),
        }
    }
}
#[cfg(not(target_family = "wasm"))]
pub fn system_languages() -> Vec<String> {
    sys_locale::get_locales().collect()
}
#[cfg(target_family = "wasm")]
pub fn system_languages() -> Vec<String> {
    Vec::new()
}

type Bundle = FluentBundle<FluentResource>;
static BUNDLES: OnceLock<Mutex<HashMap<Locale, Arc<Bundle>>>> = OnceLock::new();
fn bundle(locale: Locale) -> Arc<Bundle> {
    let mut cache = BUNDLES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    cache
        .entry(locale)
        .or_insert_with(|| {
            let mut bundle =
                Bundle::new_concurrent(vec![locale.code().parse().expect("registered locale")]);
            // All shipped languages are LTR. Isolate user-supplied interpolation values
            // using Fluent's default bidi isolation, including paths and mixed scripts.
            for source in resources(locale) {
                let resource = FluentResource::try_new((*source).into())
                    .expect("build-validated Fluent resource");
                bundle.add_resource(resource).expect("unique message keys");
            }
            Arc::new(bundle)
        })
        .clone()
}
struct Presentation {
    number: icu_decimal::DecimalFormatter,
    date: icu_datetime::DateTimeFormatter<icu_datetime::fieldsets::YMD>,
}
fn presentation(locale: Locale) -> Arc<Presentation> {
    static CACHE: OnceLock<Mutex<HashMap<Locale, Arc<Presentation>>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    cache
        .entry(locale)
        .or_insert_with(|| {
            let locale: icu_locale::Locale = locale.code().parse().expect("registered locale");
            Arc::new(Presentation {
                number: icu_decimal::DecimalFormatter::try_new(
                    locale.clone().into(),
                    Default::default(),
                )
                .expect("bundled decimal data"),
                date: icu_datetime::DateTimeFormatter::try_new(
                    locale.into(),
                    icu_datetime::fieldsets::YMD::medium(),
                )
                .expect("bundled date data"),
            })
        })
        .clone()
}

#[derive(Clone)]
pub struct Localizer {
    locale: Locale,
    bundle: Arc<Bundle>,
    english: Arc<Bundle>,
    presentation: Arc<Presentation>,
}
impl Default for Localizer {
    fn default() -> Self {
        Self::new(Locale::En)
    }
}
impl Localizer {
    pub fn new(locale: Locale) -> Self {
        Self {
            locale,
            bundle: bundle(locale),
            english: bundle(Locale::En),
            presentation: presentation(locale),
        }
    }
    /// Development-only expanded English. Interpolated values and rich markers are preserved.
    #[cfg(debug_assertions)]
    pub fn expanded() -> Self {
        static PSEUDO: OnceLock<Arc<Bundle>> = OnceLock::new();
        let pseudo = PSEUDO
            .get_or_init(|| {
                let mut bundle = Bundle::new_concurrent(vec!["en".parse().unwrap()]);
                for source in pseudo_resources() {
                    bundle
                        .add_resource(FluentResource::try_new((*source).into()).unwrap())
                        .unwrap();
                }
                Arc::new(bundle)
            })
            .clone();
        Self {
            locale: Locale::En,
            bundle: pseudo,
            english: bundle(Locale::En),
            presentation: presentation(Locale::En),
        }
    }
    pub fn locale(&self) -> Locale {
        self.locale
    }
    pub fn text(&self, id: &str) -> String {
        self.format(id, None)
    }
    pub fn format(&self, id: &str, args: Option<&FluentArgs<'_>>) -> String {
        for bundle in [&self.bundle, &self.english] {
            if let Some(pattern) = bundle.get_message(id).and_then(|message| message.value()) {
                let mut errors = Vec::new();
                let value = bundle.format_pattern(pattern, args, &mut errors);
                if errors.is_empty() {
                    return value.into_owned();
                }
            }
        }
        id.to_owned()
    }
    pub fn attribute(&self, id: &str, attribute: &str, args: Option<&FluentArgs<'_>>) -> String {
        for bundle in [&self.bundle, &self.english] {
            if let Some(message) = bundle.get_message(id) {
                if let Some(pattern) = message.get_attribute(attribute) {
                    let mut errors = Vec::new();
                    let value = bundle.format_pattern(pattern.value(), args, &mut errors);
                    if errors.is_empty() {
                        return value.into_owned();
                    }
                }
            }
        }
        format!("{id}.{attribute}")
    }
}
include!(concat!(env!("OUT_DIR"), "/resources.rs"));
/// Generated accessors require every English message argument at the call site.
pub mod messages {
    include!(concat!(env!("OUT_DIR"), "/messages.rs"));
}

impl Localizer {
    /// Locale presentation only: callers retain ownership of units and currencies.
    pub fn number(&self, value: i64) -> String {
        use writeable::Writeable;
        self.presentation
            .number
            .format(&icu_decimal::input::Decimal::from(value))
            .write_to_string()
            .into_owned()
    }

    /// Formats a Gregorian civil date; does not reinterpret time zones or timestamps.
    pub fn date(&self, year: i32, month: u8, day: u8) -> Option<String> {
        use icu_datetime::input::Date;
        use writeable::Writeable;
        let date = Date::try_new_iso(year, month, day).ok()?;
        Some(
            self.presentation
                .date
                .format(&date)
                .write_to_string()
                .into_owned(),
        )
    }
}

#[cfg(test)]
mod tests;
