//! Motion policy shared by desktop and browser presentation.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReduceMotion {
    #[default]
    System,
    On,
    Off,
}
impl ReduceMotion {
    pub const ALL: [Self; 3] = [Self::System, Self::On, Self::Off];
    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::On => "On",
            Self::Off => "Off",
        }
    }
}
/// Background pause applies independently of the system/user motion preference.
pub fn resolve(
    preference: ReduceMotion,
    system: bool,
    pause_in_background: bool,
    active: bool,
) -> bool {
    let reduced = match preference {
        ReduceMotion::System => system,
        ReduceMotion::On => true,
        ReduceMotion::Off => false,
    };
    reduced || (pause_in_background && !active)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn motion_policy_preserves_wire_values_and_background_override() {
        for (preference, system, pause, active, expected) in [
            (ReduceMotion::System, false, false, true, false),
            (ReduceMotion::System, true, false, true, true),
            (ReduceMotion::On, false, false, true, true),
            (ReduceMotion::Off, true, false, true, false),
            (ReduceMotion::Off, true, true, false, true),
            (ReduceMotion::On, false, true, true, true),
        ] {
            assert_eq!(resolve(preference, system, pause, active), expected);
        }
        assert_eq!(
            serde_json::to_value(ReduceMotion::System).unwrap(),
            "system"
        );
        let old: super::super::WorkspacePreferences = serde_json::from_str("{}").unwrap();
        assert_eq!(old.reduce_motion, ReduceMotion::System);
        assert!(!old.pause_animations_in_background);
    }
}
