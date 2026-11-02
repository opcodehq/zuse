//! Authored completion cues shared by native PCM playback and Web Audio.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompletionSoundPreset {
    #[default]
    Chime,
    Soft,
    Pop,
    Bell,
    Rise,
    Bloom,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompletionSoundPreferences {
    pub enabled: bool,
    pub preset: CompletionSoundPreset,
}
impl Default for CompletionSoundPreferences {
    fn default() -> Self {
        Self {
            enabled: true,
            preset: CompletionSoundPreset::Chime,
        }
    }
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Wave {
    Sine,
    Triangle,
}
#[derive(Clone, Copy, Serialize)]
pub struct Tone {
    pub offset: f32,
    pub frequency: f32,
    pub duration: f32,
    pub gain: f32,
    pub wave: Wave,
}
const fn tone(offset: f32, frequency: f32, duration: f32, gain: f32) -> Tone {
    Tone {
        offset,
        frequency,
        duration,
        gain,
        wave: Wave::Sine,
    }
}
impl CompletionSoundPreset {
    pub const ALL: [Self; 6] = [
        Self::Chime,
        Self::Soft,
        Self::Pop,
        Self::Bell,
        Self::Rise,
        Self::Bloom,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Chime => "Chime",
            Self::Soft => "Soft",
            Self::Pop => "Pop",
            Self::Bell => "Bell",
            Self::Rise => "Rise",
            Self::Bloom => "Bloom",
        }
    }
    pub fn tones(self) -> &'static [Tone] {
        const CHIME: [Tone; 2] = [
            tone(0., 587.33, 0.18, 0.055),
            tone(0.12, 783.99, 0.28, 0.046),
        ];
        const SOFT: [Tone; 2] = [
            tone(0., 523.25, 0.24, 0.048),
            tone(0.12, 659.25, 0.32, 0.04),
        ];
        const POP: [Tone; 2] = [
            Tone {
                wave: Wave::Triangle,
                ..tone(0., 740., 0.12, 0.06)
            },
            Tone {
                wave: Wave::Triangle,
                ..tone(0.08, 987.77, 0.14, 0.034)
            },
        ];
        const BELL: [Tone; 2] = [tone(0., 880., 0.22, 0.052), tone(0.04, 1320., 0.42, 0.024)];
        const RISE: [Tone; 3] = [
            tone(0., 440., 0.16, 0.04),
            tone(0.11, 554.37, 0.18, 0.044),
            tone(0.23, 659.25, 0.24, 0.04),
        ];
        const BLOOM: [Tone; 3] = [
            tone(0., 392., 0.34, 0.034),
            tone(0.04, 493.88, 0.38, 0.035),
            tone(0.11, 659.25, 0.34, 0.032),
        ];
        match self {
            Self::Chime => &CHIME,
            Self::Soft => &SOFT,
            Self::Pop => &POP,
            Self::Bell => &BELL,
            Self::Rise => &RISE,
            Self::Bloom => &BLOOM,
        }
    }
    /// Mono 16-bit PCM WAV for the platform players. Envelopes follow the
    /// same 12ms exponential attack and exponential decay as the Web Audio cue.
    pub fn wav(self) -> Vec<u8> {
        const RATE: u32 = 44_100;
        let tones = self.tones();
        let duration = tones
            .iter()
            .map(|tone| tone.offset + tone.duration + 0.02)
            .fold(0., f32::max);
        let samples = (duration * RATE as f32).ceil() as u32;
        let bytes = samples * 2;
        let mut wav = Vec::with_capacity(44 + bytes as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + bytes).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&RATE.to_le_bytes());
        wav.extend_from_slice(&(RATE * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&bytes.to_le_bytes());
        for sample in 0..samples {
            let time = sample as f32 / RATE as f32;
            let mut value = 0.;
            for tone in tones {
                let time = time - tone.offset;
                if time < 0. || time > tone.duration {
                    continue;
                }
                let envelope = if time < 0.012 {
                    0.0001 * (tone.gain / 0.0001).powf(time / 0.012)
                } else {
                    tone.gain * (0.0001 / tone.gain).powf((time - 0.012) / (tone.duration - 0.012))
                };
                let sine = (std::f32::consts::TAU * tone.frequency * time).sin();
                let wave = match tone.wave {
                    Wave::Sine => sine,
                    Wave::Triangle => 2. / std::f32::consts::PI * sine.asin(),
                };
                value += wave * envelope;
            }
            wav.extend_from_slice(
                &((value.clamp(-1., 1.) * i16::MAX as f32).round() as i16).to_le_bytes(),
            );
        }
        wav
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_preset_encodes_a_distinct_bounded_playable_pcm_cue() {
        let cues: Vec<_> = CompletionSoundPreset::ALL
            .into_iter()
            .map(CompletionSoundPreset::wav)
            .collect();
        for (index, cue) in cues.iter().enumerate() {
            assert_eq!(&cue[..4], b"RIFF");
            assert_eq!(&cue[8..16], b"WAVEfmt ");
            assert_eq!(
                u32::from_le_bytes(cue[4..8].try_into().unwrap()) as usize + 8,
                cue.len()
            );
            assert_eq!(
                u32::from_le_bytes(cue[40..44].try_into().unwrap()) as usize + 44,
                cue.len()
            );
            assert!(cue.len() > 4_400 && cue.len() < 88_244);
            assert!(
                cue[44..].chunks_exact(2).any(|sample| i16::from_le_bytes(
                    sample.try_into().unwrap()
                )
                .abs()
                    > 100)
            );
            assert!(cues[..index].iter().all(|previous| previous != cue));
        }
    }
}
