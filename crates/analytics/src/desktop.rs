//! Finite native UI vocabulary and transition bookkeeping. No entity identifiers.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{Duration, Instant};

/// Transport envelope. The epoch never enters the public event payload.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    #[serde(default)]
    pub collection_epoch: String,
    pub event: Event,
    #[serde(default)]
    pub client: Option<Client>,
}

/// Client facts are explicit; the connected engine may run on another OS/version.
#[derive(Serialize, Deserialize)]
pub struct Client {
    pub os: ClientOs,
    pub architecture: ClientArchitecture,
    pub version: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientOs {
    Macos,
    Windows,
    Linux,
    Other,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientArchitecture {
    Aarch64,
    X86_64,
    Other,
}

impl Client {
    pub fn current() -> Self {
        Self {
            os: match std::env::consts::OS {
                "macos" => ClientOs::Macos,
                "windows" => ClientOs::Windows,
                "linux" => ClientOs::Linux,
                _ => ClientOs::Other,
            },
            architecture: match std::env::consts::ARCH {
                "aarch64" => ClientArchitecture::Aarch64,
                "x86_64" => ClientArchitecture::X86_64,
                _ => ClientArchitecture::Other,
            },
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }

    pub(crate) fn properties(&self) -> Value {
        let mut properties = serde_json::json!({
            "client_os": self.os, "client_architecture": self.architecture,
        });
        // Public workspace package versions are numeric. Custom build labels stay out.
        let parts: Vec<_> = self.version.split('.').collect();
        if self.version.len() <= 32
            && parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        {
            properties["client_version"] = serde_json::json!(self.version);
        }
        properties
    }
}

macro_rules! catalog {
    ($name:ident { $($(#[$meta:meta])* $variant:ident),* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($(#[$meta])* $variant),* }
    };
}
catalog!(Screen {
    Chat,
    PullRequests,
    Plugins,
    Archives,
    Usage,
    // Recorded as `zot`, its name before the rename, so the route's history
    // stays one series in analytics. Legacy value; "zapper" is accepted too.
    #[serde(rename = "zot", alias = "zapper")]
    Zapper,
    Settings
});
catalog!(SettingsPage {
    Cloud,
    CloudRepositories,
    CloudEnvironments,
    CloudMachine,
    CloudAgents,
    Usage,
    Billing,
    Devices,
    Harnesses,
    Defaults,
    Plugins,
    Agents,
    Appearance,
    Files,
    Notifications,
    Voice,
    Shortcuts,
    General,
    Appshots,
    Archived
});
catalog!(Panel { Sidebar, Surfaces });
catalog!(Surface {
    Picker,
    File,
    Browser,
    Simulator,
    Desktop,
    Diff,
    Terminal,
    Subagent,
    SideChat,
    PullRequest,
    Files
});
catalog!(PaletteTab {
    All,
    Threads,
    Commands,
    Files
});
catalog!(PaletteAction {
    NewChat,
    NewProject,
    Settings,
    Theme,
    ArchiveThread,
    Chat,
    File,
    Folder,
    Content
});
catalog!(Preference {
    FileWordWrap,
    ShowAllFiles,
    Typography,
    Appearance,
    Language,
    Composer,
    Notifications,
    Sound,
    Dictation,
    Shortcuts,
    Sidebar,
    Browser,
    Diff,
    Transcript,
    FileAutosave,
    Appshots,
    DefaultModel,
    DefaultPermissions
});
catalog!(OnboardingStep {
    Welcome,
    Agents,
    SignIn,
    Plugins,
    Chats,
    Personalize,
    Launch
});
catalog!(OnboardingOutcome { Completed, Skipped });

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "properties")]
pub enum Event {
    #[serde(rename = "onboarding step viewed")]
    OnboardingStepViewed { step: OnboardingStep },
    #[serde(rename = "onboarding finished")]
    OnboardingFinished { outcome: OnboardingOutcome },
    #[serde(rename = "app opened")]
    Opened,
    #[serde(rename = "app closed")]
    Closed,
    #[serde(rename = "app foregrounded")]
    Foregrounded,
    #[serde(rename = "app backgrounded")]
    Backgrounded,
    #[serde(rename = "app active interval")]
    ActiveInterval { active_seconds: u64 },
    #[serde(rename = "screen viewed")]
    ScreenViewed { screen: Screen },
    #[serde(rename = "settings viewed")]
    SettingsViewed { section: SettingsPage },
    #[serde(rename = "panel changed")]
    PanelChanged { panel: Panel, open: bool },
    #[serde(rename = "surface selected")]
    SurfaceSelected { surface_kind: Surface },
    #[serde(rename = "palette opened")]
    PaletteOpened,
    #[serde(rename = "palette tab changed")]
    PaletteTabChanged { tab: PaletteTab },
    #[serde(rename = "palette action invoked")]
    PaletteActionInvoked { action: PaletteAction },
    #[serde(rename = "preference changed")]
    PreferenceChanged { preference: Preference },
}
impl Event {
    pub fn parts(&self) -> (String, Value) {
        let value = serde_json::to_value(self).expect("finite event serializes");
        (
            value["event"].as_str().unwrap().into(),
            value
                .get("properties")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
        )
    }
    pub fn valid(&self) -> bool {
        !matches!(self, Self::ActiveInterval { active_seconds } if !(1..=60).contains(active_seconds))
    }
}

/// One native workspace-window state. Time is monotonic; gaps are capped
/// at 60 seconds so suspension cannot produce hours of spurious active usage.
#[derive(Default)]
pub struct Lifecycle {
    opened: bool,
    closed: bool,
    foreground: bool,
    since: Option<Instant>,
    screen: Option<(Screen, Option<SettingsPage>)>,
    onboarding: Option<OnboardingStep>,
}
impl Lifecycle {
    /// Snapshot the current route and visible onboarding step when a binding becomes ready.
    pub fn ready(&mut self) -> Vec<Event> {
        if !self.opened {
            return self.open();
        }
        let mut events = Vec::new();
        if let Some((screen, section)) = self.screen {
            events.push(Event::ScreenViewed { screen });
            if let Some(section) = section {
                events.push(Event::SettingsViewed { section });
            }
        }
        if let Some(step) = self.onboarding {
            events.push(Event::OnboardingStepViewed { step });
        }
        events
    }
    pub fn open(&mut self) -> Vec<Event> {
        if self.opened || self.closed {
            return vec![];
        }
        self.opened = true;
        let mut events = vec![Event::Opened];
        if self.foreground {
            events.push(Event::Foregrounded);
        }
        if let Some((screen, section)) = self.screen {
            events.push(Event::ScreenViewed { screen });
            if let Some(section) = section {
                events.push(Event::SettingsViewed { section });
            }
        }
        if let Some(step) = self.onboarding {
            events.push(Event::OnboardingStepViewed { step });
        }
        events
    }
    pub fn onboarding(&mut self, step: Option<OnboardingStep>) -> Vec<Event> {
        if self.closed || self.onboarding == step {
            return vec![];
        }
        self.onboarding = step;
        step.map(|step| vec![Event::OnboardingStepViewed { step }])
            .unwrap_or_default()
    }
    pub fn screen(&mut self, screen: Screen, section: Option<SettingsPage>) -> Vec<Event> {
        let next = (screen, section);
        if self.screen == Some(next) {
            return vec![];
        }
        let changed_screen = self.screen.map(|previous| previous.0) != Some(screen);
        self.screen = Some(next);
        let mut events = Vec::new();
        if changed_screen {
            events.push(Event::ScreenViewed { screen });
        }
        if let Some(section) = section {
            events.push(Event::SettingsViewed { section });
        }
        events
    }
    pub fn foreground(&mut self, active: bool, now: Instant) -> Vec<Event> {
        if self.closed || self.foreground == active {
            return vec![];
        }
        let mut events = self.interval(now);
        self.foreground = active;
        self.since = active.then_some(now);
        events.push(if active {
            Event::Foregrounded
        } else {
            Event::Backgrounded
        });
        events
    }
    pub fn interval(&mut self, now: Instant) -> Vec<Event> {
        if !self.foreground || self.closed {
            return vec![];
        }
        let seconds = self
            .since
            .map(|since| {
                now.saturating_duration_since(since)
                    .min(Duration::from_secs(60))
                    .as_secs()
            })
            .unwrap_or(0);
        self.since = Some(now);
        if seconds == 0 {
            vec![]
        } else {
            vec![Event::ActiveInterval {
                active_seconds: seconds,
            }]
        }
    }
    /// Rebinding never replays navigation or time from the previous engine.
    pub fn rebind(&mut self, now: Instant) {
        self.since = self.foreground.then_some(now);
    }
    pub fn close(&mut self, now: Instant) -> Vec<Event> {
        if !self.opened || self.closed {
            return vec![];
        }
        let mut events = self.interval(now);
        self.closed = true;
        events.push(Event::Closed);
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_snapshots_only_the_current_visible_onboarding_step() {
        let mut lifecycle = Lifecycle::default();
        lifecycle.onboarding(Some(OnboardingStep::Welcome));
        lifecycle.onboarding(Some(OnboardingStep::Agents));
        assert_eq!(
            lifecycle.ready(),
            vec![
                Event::Opened,
                Event::OnboardingStepViewed {
                    step: OnboardingStep::Agents
                }
            ]
        );
        assert!(
            lifecycle
                .onboarding(Some(OnboardingStep::Agents))
                .is_empty()
        );
        lifecycle.onboarding(None);
        assert!(lifecycle.ready().is_empty());
    }
    #[test]
    fn transitions_deduplicate_and_cap_sleep() {
        let mut lifecycle = Lifecycle::default();
        let now = Instant::now();
        assert_eq!(lifecycle.open(), vec![Event::Opened]);
        assert!(lifecycle.open().is_empty());
        assert_eq!(lifecycle.foreground(true, now), vec![Event::Foregrounded]);
        assert!(lifecycle.foreground(true, now).is_empty());
        assert_eq!(
            lifecycle.foreground(false, now + Duration::from_secs(3600)),
            vec![
                Event::ActiveInterval { active_seconds: 60 },
                Event::Backgrounded
            ]
        );
        assert!(
            lifecycle
                .interval(now + Duration::from_secs(7200))
                .is_empty()
        );
        assert_eq!(lifecycle.close(now), vec![Event::Closed]);
        assert!(lifecycle.close(now).is_empty());
        assert!(lifecycle.foreground(true, now).is_empty());
    }
    #[test]
    fn navigation_counts_visits_and_sections_not_renders() {
        let mut lifecycle = Lifecycle::default();
        assert_eq!(
            lifecycle
                .screen(Screen::Settings, Some(SettingsPage::General))
                .len(),
            2
        );
        assert!(
            lifecycle
                .screen(Screen::Settings, Some(SettingsPage::General))
                .is_empty()
        );
        assert_eq!(
            lifecycle.screen(Screen::Settings, Some(SettingsPage::Files)),
            vec![Event::SettingsViewed {
                section: SettingsPage::Files
            }]
        );
        lifecycle.screen(Screen::Chat, None);
        assert_eq!(
            lifecycle
                .screen(Screen::Settings, Some(SettingsPage::Files))
                .len(),
            2
        );
    }
    #[test]
    fn rebind_discards_old_engine_time() {
        let mut lifecycle = Lifecycle::default();
        let now = Instant::now();
        lifecycle.foreground(true, now);
        lifecycle.rebind(now + Duration::from_secs(50));
        assert_eq!(
            lifecycle.interval(now + Duration::from_secs(55)),
            vec![Event::ActiveInterval { active_seconds: 5 }]
        );
    }
}
