//! Deterministic barriers exercise the production capture/phrase coordinator.
use super::*;
use std::{
    sync::mpsc::{self, Receiver, SyncSender},
    thread::JoinHandle,
};

const WAIT: Duration = Duration::from_secs(5);
const RATE: u32 = 16_000;

/// A microphone the test speaks into: `level` is the meter reading and
/// `pending` the audio since the last cut.
#[derive(Default)]
struct Mic {
    level: f32,
    pending: Vec<f32>,
    cuts: usize,
}
struct FakeCapture {
    mic: Arc<Mutex<Mic>>,
    closed: SyncSender<()>,
    ended: Arc<AtomicBool>,
}
impl Recording for FakeCapture {
    fn ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }
    fn rate(&self) -> u32 {
        RATE
    }
    fn take_level(&self) -> f32 {
        self.mic.lock().unwrap().level
    }
    fn cut(&self) -> Vec<f32> {
        let mut mic = self.mic.lock().unwrap();
        mic.cuts += 1;
        std::mem::take(&mut mic.pending)
    }
    fn finish(self) -> Result<Vec<f32>> {
        Ok(std::mem::take(&mut self.mic.lock().unwrap().pending))
    }
}
impl Drop for FakeCapture {
    fn drop(&mut self) {
        let _ = self.closed.try_send(());
    }
}

/// A phrase upload the test answers: the first sample identifies the phrase.
type Upload = (f32, SyncSender<Result<String>>);

struct Fixture {
    session: Session,
    mic: Arc<Mutex<Mic>>,
    closed: Receiver<()>,
    ended: Arc<AtomicBool>,
    uploads: Receiver<Upload>,
    worker: JoinHandle<()>,
}
impl Fixture {
    fn new(open: Option<(Receiver<()>, SyncSender<()>)>) -> Self {
        Self::with_admission(open, None)
    }
    fn with_admission(
        open: Option<(Receiver<()>, SyncSender<()>)>,
        admission: Option<BusyGuard>,
    ) -> Self {
        let (events_tx, events) = mpsc::sync_channel(EVENT_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let job = Job {
            stop: stop.clone(),
            cancel: cancel.clone(),
            level: level.clone(),
            device: None,
            events: events_tx,
        };
        let mic = Arc::new(Mutex::new(Mic::default()));
        let (closed_tx, closed) = mpsc::sync_channel(1);
        let (uploads_tx, uploads) = mpsc::sync_channel::<Upload>(8);
        let ended = Arc::new(AtomicBool::new(false));
        let (fake_mic, end_capture) = (mic.clone(), ended.clone());
        let worker = std::thread::spawn(move || {
            let _admission = admission;
            let uploads = Mutex::new(uploads_tx);
            let result = run_job(
                &job,
                || {
                    if let Some((open, opening)) = open {
                        opening.send(()).unwrap();
                        open.recv_timeout(WAIT).unwrap();
                    }
                    Ok(FakeCapture {
                        mic: fake_mic,
                        closed: closed_tx,
                        ended: end_capture,
                    })
                },
                |samples, rate| {
                    assert_eq!(rate, RATE);
                    let (reply, answer) = mpsc::sync_channel(1);
                    let sent = uploads.lock().unwrap().send((samples[0], reply));
                    sent.map_err(|_| anyhow::anyhow!("test ended"))?;
                    // Cancellation aborts a pending upload, as the RPC does.
                    loop {
                        match answer.recv_timeout(Duration::from_millis(10)) {
                            Ok(result) => return result,
                            Err(_) if job.cancel.load(Ordering::Acquire) => {
                                anyhow::bail!("cancelled")
                            }
                            Err(_) => {}
                        }
                    }
                },
            );
            job.finished(result);
        });
        Self {
            session: Session {
                stop,
                cancel,
                level,
                events,
            },
            mic,
            closed,
            ended,
            uploads,
            worker,
        }
    }
    /// Records `seconds` of audio at `amplitude`, which also identifies it.
    fn speak(&self, amplitude: f32, seconds: f32, level: f32) {
        let mut mic = self.mic.lock().unwrap();
        mic.level = level;
        mic.pending.extend(std::iter::repeat_n(
            amplitude,
            (RATE as f32 * seconds) as usize,
        ));
    }
    fn quiet(&self) {
        self.mic.lock().unwrap().level = 0.0;
    }
    fn next(&self) -> Event {
        self.session.events.recv_timeout(WAIT).unwrap()
    }
    fn upload(&self) -> Upload {
        self.uploads.recv_timeout(WAIT).unwrap()
    }
    fn listening(&self) {
        assert!(matches!(self.next(), Event::Listening));
    }
    fn end(self) -> Vec<Event> {
        self.worker.join().unwrap();
        self.session.events.try_iter().collect()
    }
}
fn text(event: Event) -> String {
    match event {
        Event::Partial(text) | Event::Final(text) => text,
        other => panic!("expected text, got {other:?}"),
    }
}

#[test]
fn phrase_cutter_waits_for_a_pause_after_enough_speech() {
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let mut cutter = PhraseCutter::new(t0);
    // Silence before speech never ends a phrase.
    for ms in (100..=3_000).step_by(100) {
        assert!(!cutter.poll(at(ms), 0.0));
    }
    let mut cutter = PhraseCutter::new(t0);
    assert!(!cutter.poll(at(100), SPEECH_RMS));
    // 800 ms of quiet, but the phrase is still shorter than 1.5 s.
    assert!(!cutter.poll(at(200), 0.0));
    assert!(!cutter.poll(at(1_000), 0.0));
    assert!(cutter.poll(at(1_500), 0.0));
    // The next phrase starts fresh at the cut.
    assert!(!cutter.heard_speech);
    assert!(!cutter.poll(at(3_100), 0.0));
}

#[test]
fn phrase_cutter_keeps_short_gaps_between_words_together() {
    let t0 = Instant::now();
    let mut cutter = PhraseCutter::new(t0);
    let mut ms = 0;
    for _word in 0..6 {
        for _ in 0..4 {
            ms += 100;
            assert!(!cutter.poll(t0 + Duration::from_millis(ms), 1.0));
        }
        for _ in 0..7 {
            ms += 100;
            assert!(!cutter.poll(t0 + Duration::from_millis(ms), 0.0));
        }
    }
    // The last gap grows into a pause: 800 ms after it began, the phrase ends.
    assert!(!cutter.poll(t0 + Duration::from_millis(ms + 100), 0.0));
    assert!(cutter.poll(t0 + Duration::from_millis(ms + 200), 0.0));
}

#[test]
fn delivery_keeps_spoken_order_when_responses_race() {
    let mut delivery = Delivery::default();
    let (first, second, third) = (delivery.reserve(), delivery.reserve(), delivery.reserve());
    assert_eq!(delivery.arrive(third, "third".into()), None);
    assert_eq!(delivery.arrive(second, " ".into()), None);
    assert!(delivery.pending());
    assert_eq!(
        delivery.arrive(first, " first ".into()),
        Some("first third")
    );
    assert!(!delivery.pending());
}

#[test]
fn only_the_first_phrase_is_sent_without_heard_speech() {
    let speech = vec![0.25; RATE as usize];
    assert!(worth_sending(&speech, RATE, true, false));
    assert!(worth_sending(&speech, RATE, false, true));
    assert!(!worth_sending(&speech, RATE, false, false));
    assert!(!worth_sending(
        &speech[..RATE as usize / 10],
        RATE,
        true,
        true
    ));
    assert!(!worth_sending(&vec![0.0; RATE as usize], RATE, true, true));
}

#[test]
fn phrases_transcribe_while_speaking_and_stop_waits_for_the_last() {
    let mut f = Fixture::new(None);
    f.listening();
    f.speak(0.25, 2.0, 1.0);
    std::thread::sleep(Duration::from_millis(1_600));
    f.quiet();
    let (first, reply) = f.upload();
    assert_eq!(first, 0.25);
    assert_eq!(f.mic.lock().unwrap().cuts, 1);
    reply.send(Ok("first thought".into())).unwrap();
    assert_eq!(text(f.next()), "first thought");
    // The microphone is still open: this phrase belongs to the same take.
    assert!(f.closed.try_recv().is_err());
    f.speak(0.5, 0.5, 1.0);
    f.session.finish();
    f.closed.recv_timeout(WAIT).unwrap();
    assert!(matches!(f.next(), Event::Finalizing));
    let (second, reply) = f.upload();
    assert_eq!(second, 0.5);
    reply.send(Ok("second thought".into())).unwrap();
    assert_eq!(text(f.next()), "first thought second thought");
    assert_eq!(text(f.next()), "first thought second thought");
    assert!(f.end().is_empty());
}

#[test]
fn racing_phrases_deliver_in_spoken_order() {
    let mut f = Fixture::new(None);
    f.listening();
    f.speak(0.25, 2.0, 1.0);
    std::thread::sleep(Duration::from_millis(1_600));
    f.quiet();
    let (_, earlier) = f.upload();
    f.speak(0.5, 0.5, 1.0);
    f.session.finish();
    let (_, later) = f.upload();
    assert!(matches!(f.next(), Event::Finalizing));
    later.send(Ok("later".into())).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(f.session.poll().is_none(), "a later phrase must wait");
    earlier.send(Ok("earlier".into())).unwrap();
    assert_eq!(text(f.next()), "earlier later");
    assert!(matches!(f.next(), Event::Final(text) if text == "earlier later"));
    f.end();
}

#[test]
fn silent_trailing_phrase_is_not_sent() {
    let mut f = Fixture::new(None);
    f.listening();
    f.speak(0.25, 2.0, 1.0);
    std::thread::sleep(Duration::from_millis(1_600));
    f.quiet();
    let (_, reply) = f.upload();
    reply.send(Ok("loud part".into())).unwrap();
    assert_eq!(text(f.next()), "loud part");
    f.speak(0.01, 2.0, 0.0);
    f.session.finish();
    assert!(matches!(f.next(), Event::Finalizing));
    assert!(matches!(f.next(), Event::Final(text) if text == "loud part"));
    assert!(f.uploads.try_recv().is_err());
    f.end();
}

#[test]
fn first_phrase_is_sent_even_when_the_meter_heard_nothing() {
    let mut f = Fixture::new(None);
    f.listening();
    f.speak(0.01, 1.0, 0.0);
    f.session.finish();
    let (_, reply) = f.upload();
    reply.send(Ok(String::new())).unwrap();
    assert!(matches!(f.next(), Event::Finalizing));
    // Audio arrived but no words: the UI reports no speech.
    assert!(matches!(f.next(), Event::Final(text) if text.is_empty()));
    f.end();
}

#[test]
fn no_audio_is_reported_apart_from_no_speech() {
    let mut f = Fixture::new(None);
    f.listening();
    f.session.finish();
    assert!(matches!(f.next(), Event::Finalizing));
    assert!(matches!(f.next(), Event::NoAudio));
    assert!(f.uploads.try_recv().is_err());
    f.end();
}

#[test]
fn failed_phrase_ends_dictation_before_earlier_phrases_land() {
    let mut f = Fixture::new(None);
    f.listening();
    f.speak(0.25, 2.0, 1.0);
    std::thread::sleep(Duration::from_millis(1_600));
    f.quiet();
    let (_, earlier) = f.upload();
    f.speak(0.5, 0.5, 1.0);
    f.session.finish();
    let (_, later) = f.upload();
    assert!(matches!(f.next(), Event::Finalizing));
    later.send(Err(anyhow::anyhow!("offline"))).unwrap();
    assert!(matches!(f.next(), Event::Failed(message) if message == "offline"));
    // The earlier upload was aborted, so its text never arrives.
    let _ = earlier.send(Ok("arrives after the failure".into()));
    assert!(f.end().is_empty());
}

#[test]
fn cancel_drops_only_audio_not_yet_transcribed() {
    let f = Fixture::new(None);
    f.listening();
    f.speak(0.25, 2.0, 1.0);
    std::thread::sleep(Duration::from_millis(1_600));
    f.quiet();
    let (_, reply) = f.upload();
    reply.send(Ok("kept".into())).unwrap();
    assert_eq!(text(f.next()), "kept");
    f.speak(0.5, 1.0, 1.0);
    f.session.cancel.store(true, Ordering::Release);
    f.closed.recv_timeout(WAIT).unwrap();
    assert!(f.uploads.try_recv().is_err());
    assert!(f.end().is_empty());
}

#[test]
fn cancel_during_upload_closes_microphone_and_reports_nothing() {
    let mut f = Fixture::new(None);
    f.listening();
    f.speak(0.25, 1.0, 1.0);
    f.session.finish();
    let (_, reply) = f.upload();
    f.closed.recv_timeout(WAIT).unwrap();
    let Fixture {
        session, worker, ..
    } = f;
    drop(session);
    let _ = reply.send(Ok("too late".into()));
    worker.join().unwrap();
}

#[test]
fn listening_is_not_emitted_until_device_opens_and_stop_during_open_is_safe() {
    let (open, opened) = mpsc::sync_channel(1);
    let (opening, started) = mpsc::sync_channel(1);
    let mut f = Fixture::new(Some((opened, opening)));
    started.recv_timeout(WAIT).unwrap();
    assert!(f.session.poll().is_none());
    f.speak(0.25, 1.0, 1.0);
    f.session.finish();
    open.send(()).unwrap();
    f.closed.recv_timeout(WAIT).unwrap();
    assert!(matches!(f.next(), Event::Finalizing));
    let (_, reply) = f.upload();
    reply.send(Ok("retained speech".into())).unwrap();
    assert_eq!(text(f.next()), "retained speech");
    assert!(matches!(f.next(), Event::Final(text) if text == "retained speech"));
    f.end();
}

#[test]
fn recording_limit_closes_capture_and_transcribes_the_tail() {
    let f = Fixture::new(None);
    f.listening();
    f.speak(0.25, 1.0, 1.0);
    f.ended.store(true, Ordering::Release);
    f.closed.recv_timeout(WAIT).unwrap();
    assert!(matches!(f.next(), Event::Finalizing));
    let (_, reply) = f.upload();
    reply.send(Ok("retained speech".into())).unwrap();
    assert_eq!(text(f.next()), "retained speech");
    assert!(matches!(f.next(), Event::Final(_)));
    f.end();
}

#[test]
fn cancel_during_device_open_does_not_emit_listening_or_transcribe() {
    let (open, opened) = mpsc::sync_channel(1);
    let (opening, started) = mpsc::sync_channel(1);
    let mut f = Fixture::new(Some((opened, opening)));
    started.recv_timeout(WAIT).unwrap();
    f.session.cancel.store(true, Ordering::Release);
    open.send(()).unwrap();
    f.closed.recv_timeout(WAIT).unwrap();
    assert!(f.session.poll().is_none());
    assert!(f.end().is_empty());
}

#[test]
fn admission_stays_guarded_until_uploads_return() {
    let f = Fixture::with_admission(None, Some(BusyGuard::acquire().unwrap()));
    f.listening();
    f.speak(0.25, 1.0, 1.0);
    f.ended.store(true, Ordering::Release);
    let (_, reply) = f.upload();
    assert!(busy(), "Settings must refuse device changes mid-dictation");
    assert!(Session::start_remote(None, |_, _| panic!("no admission")).is_err());
    reply.send(Ok("done".into())).unwrap();
    f.end();
    assert!(!busy());
    // Admission is reusable and releases even when a queued command is dropped.
    let (tx, rx) = mpsc::sync_channel(1);
    tx.send(BusyGuard::acquire().unwrap()).unwrap();
    drop(rx);
    assert!(!busy());
}

#[test]
fn stop_before_capture_start_never_opens_the_microphone() {
    let (events, _) = mpsc::sync_channel(4);
    let job = Job {
        stop: Arc::new(AtomicBool::new(true)),
        cancel: Arc::new(AtomicBool::new(false)),
        level: Arc::new(AtomicU32::new(0)),
        device: None,
        events,
    };
    let result = run_job::<FakeCapture>(
        &job,
        || panic!("microphone must not open"),
        |_, _| -> Result<String> { panic!("no audio to transcribe") },
    )
    .unwrap();
    assert_eq!(result, Some(Finished::default()));
}

#[test]
fn audio_buffer_caps_recording_at_duration_limit_across_phrases() {
    let audio = Audio::with_capacity(0);
    let rate = 8_000;
    let limit = rate as usize * MAX_SECONDS;
    append(&vec![0.25_f32; limit / 2], 1, rate, &audio);
    let first = std::mem::take(&mut *audio.samples.lock().unwrap());
    assert_eq!(first.len(), limit / 2);
    append(&vec![0.25_f32; limit], 1, rate, &audio);
    assert_eq!(audio.samples.lock().unwrap().len(), limit - limit / 2);
    assert!(audio.full.load(Ordering::Acquire));
}
