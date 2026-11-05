//! In-memory microphone capture for Codex-authenticated dictation.
use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

mod resample;

pub const MAX_SECONDS: usize = 150;
/// A pause this long ends a phrase; shorter gaps fall between words.
const PAUSE: Duration = Duration::from_millis(800);
/// Phrases grow to at least this length: tiny clips transcribe poorly.
const MIN_PHRASE: Duration = Duration::from_millis(1_500);
const PAUSE_POLL: Duration = Duration::from_millis(100);
/// RMS that counts as speech rather than room noise: 0.08 on a level meter
/// that scales RMS by four so normal speaking fills it.
const SPEECH_RMS: f32 = 0.08 / 4.0;

struct Audio {
    /// The phrase being recorded. The audio callback appends; a phrase cut
    /// swaps in an empty buffer, which holds the lock only for a pointer swap.
    samples: Mutex<Vec<f32>>,
    /// Frames recorded across every phrase, for the duration limit.
    recorded: AtomicUsize,
    /// Loudest RMS since the phrase cutter last read it. The waveform reads
    /// its own meter, so neither consumer steals the other's peaks.
    phrase_level: AtomicU32,
    // Outside the lock: the capture loop polls these every 10 ms and must
    // never make the real-time callback wait.
    failed: AtomicBool,
    full: AtomicBool,
}
impl Audio {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: Mutex::new(Vec::with_capacity(capacity)),
            recorded: AtomicUsize::new(0),
            phrase_level: AtomicU32::new(0),
            failed: AtomicBool::new(false),
            full: AtomicBool::new(false),
        }
    }
}
/// Loudest RMS since the UI last read it. Non-negative `f32` bit patterns sort
/// like their values, so `fetch_max` keeps the peak without a lock.
fn meter<T: cpal::Sample>(data: &[T], channels: usize, level: &AtomicU32)
where
    f32: cpal::FromSample<T>,
{
    let frames = data.len() / channels.max(1);
    if frames == 0 {
        return;
    }
    let energy = data
        .chunks_exact(channels)
        .map(|frame| {
            let s = frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / channels as f32;
            s * s
        })
        .sum::<f32>();
    let rms = (energy / frames as f32).sqrt();
    if rms.is_finite() {
        level.fetch_max(rms.to_bits(), Ordering::Relaxed);
    }
}
fn append<T: cpal::Sample>(data: &[T], channels: usize, rate: u32, a: &Audio)
where
    f32: cpal::FromSample<T>,
{
    // A phrase cut holds the lock only to swap buffers, so spin briefly
    // rather than drop audio at the seam between two phrases.
    let mut samples = None;
    for _ in 0..1_024 {
        if let Ok(guard) = a.samples.try_lock() {
            samples = Some(guard);
            break;
        }
        std::hint::spin_loop();
    }
    let Some(mut samples) = samples else { return };
    let limit = rate as usize * MAX_SECONDS;
    let mut recorded = a.recorded.load(Ordering::Relaxed);
    for frame in data.chunks_exact(channels) {
        if recorded >= limit {
            a.full.store(true, Ordering::Release);
            break;
        }
        samples.push(frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / channels as f32);
        recorded += 1;
    }
    a.recorded.store(recorded, Ordering::Release);
}
/// A microphone the user can choose: a stable identifier and a display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDevice {
    pub id: String,
    pub name: String,
}

/// Every input device the default host reports. Enumeration can block on the
/// audio server, so call it off the UI thread.
pub fn input_devices() -> Vec<InputDevice> {
    let Ok(devices) = cpal::default_host().input_devices() else {
        return Vec::new();
    };
    let mut devices: Vec<_> = devices
        .filter_map(|device| {
            Some(InputDevice {
                id: device.id().ok()?.to_string(),
                name: device.description().ok()?.name().to_owned(),
            })
        })
        .collect();
    devices.dedup_by(|a, b| a.id == b.id);
    devices
}

/// The id of the device the system currently records from by default.
pub fn default_input_device() -> Option<String> {
    Some(
        cpal::default_host()
            .default_input_device()?
            .id()
            .ok()?
            .to_string(),
    )
}

/// The chosen device when it is connected, the system default otherwise.
fn input_device(id: Option<&str>) -> Option<cpal::Device> {
    let host = cpal::default_host();
    id.and_then(|id| id.parse::<cpal::DeviceId>().ok())
        .and_then(|id| host.device_by_id(&id))
        .or_else(|| host.default_input_device())
}

pub struct Capture {
    stream: Option<cpal::Stream>,
    audio: Arc<Audio>,
    rate: u32,
}
impl Capture {
    pub fn start(device: Option<&str>, level: Arc<AtomicU32>) -> Result<Self> {
        let device = input_device(device).context("No microphone available")?;
        let config = device.default_input_config()?;
        let rate = config.sample_rate();
        let channels = config.channels() as usize;
        let audio = Arc::new(Audio::with_capacity(rate as usize * MAX_SECONDS));
        let a = audio.clone();
        let e = audio.clone();
        let err = move |_| e.failed.store(true, Ordering::Release);
        // Preserve the device's native configuration; convert every CPAL
        // sample representation through the same bounded mono callback.
        macro_rules! stream {
            ($sample:ty) => {
                device.build_input_stream(
                    &config.into(),
                    move |d: &[$sample], _| {
                        meter(d, channels, &level);
                        meter(d, channels, &a.phrase_level);
                        append(d, channels, rate, &a)
                    },
                    err,
                    None,
                )?
            };
        }
        let stream = match config.sample_format() {
            cpal::SampleFormat::I8 => stream!(i8),
            cpal::SampleFormat::I16 => stream!(i16),
            cpal::SampleFormat::I24 => stream!(cpal::I24),
            cpal::SampleFormat::I32 => stream!(i32),
            cpal::SampleFormat::I64 => stream!(i64),
            cpal::SampleFormat::U8 => stream!(u8),
            cpal::SampleFormat::U16 => stream!(u16),
            cpal::SampleFormat::U24 => stream!(cpal::U24),
            cpal::SampleFormat::U32 => stream!(u32),
            cpal::SampleFormat::U64 => stream!(u64),
            cpal::SampleFormat::F32 => stream!(f32),
            cpal::SampleFormat::F64 => stream!(f64),
            _ => bail!("Unsupported microphone format"),
        };
        stream.play()?;
        Ok(Self {
            stream: Some(stream),
            audio,
            rate,
        })
    }
    pub fn ended(&self) -> bool {
        self.audio.full.load(Ordering::Acquire) || self.audio.failed.load(Ordering::Acquire)
    }
    /// Hands off the phrase recorded so far. The next phrase's buffer is in
    /// place before this one leaves, so no audio falls between them.
    pub fn cut(&self) -> Vec<f32> {
        let limit = self.rate as usize * MAX_SECONDS;
        let recorded = self.audio.recorded.load(Ordering::Acquire);
        // Allocate outside the lock; untouched capacity costs no memory.
        let mut next = Vec::with_capacity(limit.saturating_sub(recorded));
        let mut samples = self.audio.samples.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::swap(&mut *samples, &mut next);
        next
    }
    pub fn finish(mut self) -> Result<(Vec<f32>, u32)> {
        self.stream.take();
        if self.audio.failed.load(Ordering::Acquire) {
            bail!("Microphone disconnected. Your draft is safe.")
        }
        let mut samples = self.audio.samples.lock().unwrap_or_else(|e| e.into_inner());
        Ok((std::mem::take(&mut *samples), self.rate))
    }
}

#[derive(Debug)]
pub enum Event {
    Listening,
    Finalizing,
    /// Everything transcribed so far, in spoken order. Each phrase extends
    /// the previous text, so the latest partial replaces earlier ones.
    Partial(String),
    Final(String),
    /// The microphone delivered no audio at all.
    NoAudio,
    Failed(String),
}
/// Partials are cumulative, so dropping one under backpressure loses nothing.
const EVENT_CAPACITY: usize = 32;
struct Job {
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    device: Option<String>,
    events: std::sync::mpsc::SyncSender<Event>,
}
impl Job {
    /// Terminal events block until queued: unlike partials, they carry the
    /// outcome. A dropped session closes the channel, so this never hangs.
    fn finished(&self, result: Result<Option<Finished>>) {
        let event = match result {
            Ok(Some(finished)) if finished.recorded == 0 => Event::NoAudio,
            Ok(Some(finished)) => Event::Final(finished.text),
            Ok(None) => return,
            Err(error) => Event::Failed(error.to_string()),
        };
        let _ = self.events.send(event);
    }
}
// Capture owns its stream on a separate thread until Stop or cancellation.
trait Recording {
    fn ended(&self) -> bool;
    fn rate(&self) -> u32;
    /// Loudest RMS since the previous call.
    fn take_level(&self) -> f32;
    /// Audio since the previous cut; recording continues without a gap.
    fn cut(&self) -> Vec<f32>;
    /// Closes the microphone and returns the audio after the last cut.
    fn finish(self) -> Result<Vec<f32>>;
}
impl Recording for Capture {
    fn ended(&self) -> bool {
        self.ended()
    }
    fn rate(&self) -> u32 {
        self.rate
    }
    fn take_level(&self) -> f32 {
        f32::from_bits(self.audio.phrase_level.swap(0, Ordering::Relaxed))
    }
    fn cut(&self) -> Vec<f32> {
        self.cut()
    }
    fn finish(self) -> Result<Vec<f32>> {
        Ok(self.finish()?.0)
    }
}

/// Decides where one phrase ends: after a pause, once it holds enough audio
/// to transcribe well. Silence before any speech never ends a phrase.
struct PhraseCutter {
    started: Instant,
    quiet_since: Option<Instant>,
    heard_speech: bool,
}
impl PhraseCutter {
    fn new(now: Instant) -> Self {
        Self {
            started: now,
            quiet_since: None,
            heard_speech: false,
        }
    }
    /// Folds in the loudest level since the previous poll. True when the
    /// phrase should be cut now; the next one starts at `now`.
    fn poll(&mut self, now: Instant, rms: f32) -> bool {
        if rms >= SPEECH_RMS {
            self.heard_speech = true;
            self.quiet_since = None;
            return false;
        }
        let quiet_since = *self.quiet_since.get_or_insert(now);
        if !self.heard_speech
            || now.duration_since(quiet_since) < PAUSE
            || now.duration_since(self.started) < MIN_PHRASE
        {
            return false;
        }
        *self = Self::new(now);
        true
    }
}

/// Whether a phrase goes to the transcription service. Clips too short or
/// digitally silent to hold words never do. The first phrase is always sent,
/// so a quiet speaker the meter misses still gets text; a later phrase the
/// meter heard as silence is the trailing pause and would only invite
/// invented text.
fn worth_sending(samples: &[f32], rate: u32, first: bool, heard_speech: bool) -> bool {
    samples.len() >= rate as usize / 5
        && samples.iter().any(|s| s.abs() >= 0.0001)
        && (first || heard_speech)
}

/// How a dictation ended when it was not cancelled.
#[derive(Debug, Default, PartialEq, Eq)]
struct Finished {
    /// Every delivered phrase, in spoken order.
    text: String,
    /// Frames the microphone produced; zero means no audio arrived at all.
    recorded: usize,
}

/// Delivers phrase transcripts in spoken order, however their requests race.
#[derive(Default)]
struct Delivery {
    sent: usize,
    next: usize,
    arrived: BTreeMap<usize, String>,
    text: String,
}
impl Delivery {
    /// Reserves the next spoken-order slot.
    fn reserve(&mut self) -> usize {
        self.sent += 1;
        self.sent - 1
    }
    fn pending(&self) -> bool {
        self.next < self.sent
    }
    /// Records one phrase's text. Returns the full transcript when it grew.
    fn arrive(&mut self, index: usize, text: String) -> Option<&str> {
        self.arrived.insert(index, text);
        let before = self.text.len();
        while let Some(text) = self.arrived.remove(&self.next) {
            self.next += 1;
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            if !self.text.is_empty() {
                self.text.push(' ');
            }
            self.text.push_str(text);
        }
        (self.text.len() != before).then_some(self.text.as_str())
    }
}

fn run_job<C: Recording>(
    job: &Job,
    start: impl FnOnce() -> Result<C>,
    transcribe: impl Fn(Vec<f32>, u32) -> Result<String> + Sync,
) -> Result<Option<Finished>> {
    if job.cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    if job.stop.load(Ordering::Acquire) {
        return Ok(Some(Finished::default()));
    }
    std::thread::scope(|scope| {
        // Leaving for any reason, error or unwind included, aborts uploads
        // still in flight before the scope joins them.
        struct CancelOnDrop<'a>(&'a AtomicBool);
        impl Drop for CancelOnDrop<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let _abort_uploads = CancelOnDrop(&job.cancel);
        let capture = start()?;
        // Stop/cancel may arrive while the audio device is opening.
        if job.cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        if !job.stop.load(Ordering::Acquire) {
            let _ = job.events.try_send(Event::Listening);
        }
        let rate = capture.rate();
        let (done, results) = std::sync::mpsc::channel::<(usize, Result<String>)>();
        let mut delivery = Delivery::default();
        let mut recorded = 0;
        // Each phrase uploads on its own thread while recording goes on.
        let mut send = |samples: Vec<f32>, heard_speech: bool, delivery: &mut Delivery| {
            recorded += samples.len();
            let first = delivery.sent == 0;
            if !worth_sending(&samples, rate, first, heard_speech) {
                return Ok(());
            }
            let index = delivery.reserve();
            let done = done.clone();
            let transcribe = &transcribe;
            std::thread::Builder::new()
                .name("dictation-phrase".into())
                .spawn_scoped(scope, move || {
                    let _ = done.send((index, transcribe(samples, rate)));
                })
                .map(|_| ())
        };
        // A failed phrase ends dictation at once, before earlier ones land.
        let deliver = |delivery: &mut Delivery, wait: Option<Duration>| -> Result<()> {
            let mut next = match wait {
                Some(wait) => results.recv_timeout(wait).ok(),
                None => results.try_recv().ok(),
            };
            while let Some((index, result)) = next {
                let text = result?;
                if let Some(text) = delivery.arrive(index, text) {
                    let _ = job.events.try_send(Event::Partial(text.to_owned()));
                }
                next = results.try_recv().ok();
            }
            Ok(())
        };
        let started = Instant::now();
        let mut cutter = PhraseCutter::new(started);
        let mut polled = started;
        while !job.stop.load(Ordering::Acquire)
            && !job.cancel.load(Ordering::Acquire)
            && !capture.ended()
            && started.elapsed().as_secs() < MAX_SECONDS as u64
        {
            std::thread::sleep(Duration::from_millis(10));
            let now = Instant::now();
            if now.duration_since(polled) >= PAUSE_POLL {
                polled = now;
                if cutter.poll(now, capture.take_level()) {
                    send(capture.cut(), true, &mut delivery)?;
                }
            }
            deliver(&mut delivery, None)?;
        }
        if job.cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        // Fold in speech since the last poll so a short tail is not skipped.
        let tail_heard = cutter.heard_speech || capture.take_level() >= SPEECH_RMS;
        let tail = capture.finish()?; // Close the microphone before the last upload.
        let _ = job.events.try_send(Event::Finalizing);
        send(tail, tail_heard, &mut delivery)?;
        // Stop waits only for phrases still in flight.
        while delivery.pending() {
            if job.cancel.load(Ordering::Acquire) {
                return Ok(None);
            }
            deliver(&mut delivery, Some(Duration::from_millis(20)))?;
        }
        Ok((!job.cancel.load(Ordering::Acquire)).then(|| Finished {
            text: std::mem::take(&mut delivery.text),
            recorded,
        }))
    })
}

static BUSY: AtomicBool = AtomicBool::new(false);
// The command owns admission through capture teardown and uninterruptible
// native work. Queue failure also releases it, so rapid retries cannot wedge it.
struct BusyGuard;
impl BusyGuard {
    fn acquire() -> Result<Self> {
        if BUSY
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            bail!("Dictation is still active or finishing. Wait a moment, then try again.")
        }
        Ok(Self)
    }
}
impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}
pub struct Session {
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    events: std::sync::mpsc::Receiver<Event>,
}
impl Session {
    /// Capture locally and hand each phrase to the signed-in transcription
    /// service as soon as the speaker pauses, while recording continues.
    /// Phrases may upload concurrently; their text arrives in spoken order as
    /// `Partial` events. Dropping the session cancels capture and any upload
    /// still in flight; text already delivered stays delivered.
    pub fn start_remote(
        device: Option<String>,
        transcribe: impl Fn(Vec<u8>, Arc<AtomicBool>) -> Result<String> + Send + Sync + 'static,
    ) -> Result<Self> {
        let admission = BusyGuard::acquire()?;
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let (tx, events) = std::sync::mpsc::sync_channel(EVENT_CAPACITY);
        let job = Job {
            stop: stop.clone(),
            cancel: cancel.clone(),
            level: level.clone(),
            device,
            events: tx,
        };
        std::thread::Builder::new()
            .name("dictation-upload".into())
            .spawn(move || {
                let _admission = admission;
                let result = run_job(
                    &job,
                    || Capture::start(job.device.as_deref(), job.level.clone()),
                    |samples: Vec<f32>, rate: u32| {
                        transcribe(recording_wav(samples, rate)?, job.cancel.clone())
                    },
                );
                job.finished(result);
            })?;
        Ok(Self {
            stop,
            cancel,
            level,
            events,
        })
    }
    pub fn poll(&mut self) -> Option<Event> {
        self.events.try_recv().ok()
    }
    pub fn finish(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
    /// Peak microphone RMS (0–1) since the previous call. Level only; the
    /// samples themselves stay on the worker.
    pub fn take_level(&self) -> f32 {
        f32::from_bits(self.level.swap(0, Ordering::Relaxed))
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
/// Mono PCM16 WAV keeps the maximum recording below 5 MB. No audio files
/// are written; both recording and encoded payload are scoped to this request.
pub fn recording_wav(samples: Vec<f32>, rate: u32) -> Result<Vec<u8>> {
    anyhow::ensure!(
        (8_000..=192_000).contains(&rate),
        "Unsupported microphone sample rate"
    );
    anyhow::ensure!(
        samples.len() <= rate as usize * MAX_SECONDS,
        "Recording exceeds 150 seconds"
    );
    let samples = resample::for_model(samples, rate)?;
    let size = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(size as usize + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(size + 36).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&size.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
    }
    Ok(wav)
}

pub fn busy() -> bool {
    BUSY.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::Sample;
    #[test]
    fn native_sample_formats_preserve_levels_and_downmix_channels() {
        fn check<T: cpal::Sample + cpal::FromSample<f32>>()
        where
            f32: cpal::FromSample<T>,
        {
            let audio = Audio::with_capacity(0);
            let input: Vec<T> = [-0.5_f32, 0.5, 0.25, 0.75, 0.0, 0.0]
                .into_iter()
                .map(|s| s.to_sample::<T>())
                .collect();
            append(&input, 2, 16_000, &audio);
            assert_eq!(*audio.samples.lock().unwrap(), vec![0.0, 0.5, 0.0]);
            let level = AtomicU32::new(0);
            meter(&input, 2, &level);
            let rms = f32::from_bits(level.load(Ordering::Relaxed));
            assert!((rms - (0.25_f32 / 3.0).sqrt()).abs() < 0.01, "{rms}");
            meter(&input[4..], 2, &level);
            assert_eq!(f32::from_bits(level.load(Ordering::Relaxed)), rms);
        }
        check::<i8>();
        check::<i16>();
        check::<cpal::I24>();
        check::<i32>();
        check::<i64>();
        check::<u8>();
        check::<u16>();
        check::<cpal::U24>();
        check::<u32>();
        check::<u64>();
        check::<f32>();
        check::<f64>();
    }
    #[test]
    fn upload_wav_preserves_duration_format_and_clips_samples() {
        let wav = recording_wav(vec![2.0; 48_000], 48_000).unwrap();
        let reader = hound::WavReader::new(std::io::Cursor::new(&wav)).unwrap();
        let spec = reader.spec();
        assert_eq!(
            (spec.channels, spec.sample_rate, spec.bits_per_sample),
            (1, 16_000, 16)
        );
        assert_eq!(reader.len(), 16_000);
        assert!(
            reader
                .into_samples::<i16>()
                .all(|sample| sample.unwrap() >= 0)
        );
        assert!(recording_wav(vec![], 0).is_err());
        assert!(recording_wav(vec![0.0; 16_000 * (MAX_SECONDS + 1)], 16_000).is_err());
    }
}

#[cfg(test)]
mod session_tests;
