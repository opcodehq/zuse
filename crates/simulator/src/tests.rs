use super::*;
use async_trait::async_trait;
use std::sync::Mutex as StdMutex;
const UDID: &str = "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE";
fn png() -> Vec<u8> {
    let mut bytes = vec![0; 33];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    bytes[12..16].copy_from_slice(b"IHDR");
    bytes[16..20].copy_from_slice(&1080u32.to_be_bytes());
    bytes[20..24].copy_from_slice(&1920u32.to_be_bytes());
    bytes
}
#[derive(Default)]
struct Fake {
    calls: StdMutex<Vec<(String, Vec<String>)>>,
    started: StdMutex<bool>,
    ios_booted: StdMutex<bool>,
    unavailable: StdMutex<Vec<String>>,
    avd_override: StdMutex<Option<String>>,
}
#[async_trait]
impl Runner for Fake {
    async fn run(&self, program: &str, args: &[String], _: Duration) -> Result<Vec<u8>> {
        self.calls
            .lock()
            .unwrap()
            .push((program.into(), args.into()));
        ensure!(
            !self
                .unavailable
                .lock()
                .unwrap()
                .iter()
                .any(|p| p == program),
            "Tool missing: {program}"
        );
        let args = args.iter().map(String::as_str).collect::<Vec<_>>();
        Ok(match args.as_slice() {
            ["-list-avds"] => b"Pixel_8\nFresh_AVD\n".to_vec(),
            ["devices"] => {
                if *self.started.lock().unwrap() {
                    b"List of devices attached\nemulator-5554\tdevice\nreal-phone\tdevice\n"
                        .to_vec()
                } else {
                    b"List of devices attached\nreal-phone\tdevice\n".to_vec()
                }
            }
            ["-s", "emulator-5554", "emu", "avd", "name"] => self
                .avd_override
                .lock()
                .unwrap()
                .as_ref()
                .map(|name| format!("{name}\nOK\n").into_bytes())
                .unwrap_or_else(|| b"Pixel_8\nOK\n".to_vec()),
            [
                "-s",
                "emulator-5554",
                "shell",
                "getprop",
                "sys.boot_completed",
            ] => b"1\n".to_vec(),
            ["-s", _, "exec-out", "screencap", "-p"]
            | ["simctl", "io", _, "screenshot", "--type=png", "-"] => png(),
            ["simctl", "list", "devices", "available", "--json"] => serde_json::to_vec(
                &serde_json::json!({"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-18-0":[{"udid":UDID,"name":"iPhone","isAvailable":true,"state":if *self.ios_booted.lock().unwrap() {"Booted"} else {"Shutdown"}}]}}),
            )?,
            ["simctl", "boot", _] => {
                *self.ios_booted.lock().unwrap() = true;
                vec![]
            }
            ["simctl", "getenv", _, "SIMULATOR_MAINSCREEN_SCALE"] => b"3.0\n".to_vec(),
            _ => vec![],
        })
    }
    async fn start(&self, program: &str, args: &[String]) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((program.into(), args.into()));
        *self.started.lock().unwrap() = true;
        Ok(())
    }
}
fn controller(macos: bool) -> (Simulators, Arc<Fake>) {
    let fake = Arc::new(Fake::default());
    (
        Simulators::new(
            fake.clone(),
            Toolchain {
                macos,
                ..Toolchain::default()
            },
        ),
        fake,
    )
}
fn target(id: &str) -> Target {
    Target {
        chat_id: "chat".into(),
        device_id: id.into(),
    }
}
#[tokio::test]
async fn android_discovery_boot_capture_input_and_shutdown() {
    let (sim, fake) = controller(false);
    let t = target("android:Pixel_8");
    let state = sim.state("chat").await;
    assert!(!state.enabled);
    assert_eq!(state.devices.len(), 2);
    assert!(
        state
            .requirements
            .iter()
            .any(|r| r.platform == Platform::Ios && !r.ready)
    );
    assert!(!*fake.started.lock().unwrap());
    assert!(sim.open(&t, false).await.is_err());
    sim.configure("chat", true, true).await;
    let device = sim.open(&t, true).await.unwrap();
    assert_eq!(device.serial.as_deref(), Some("emulator-5554"));
    let frame = sim.screenshot(&t, true).await.unwrap();
    assert_eq!((frame.width, frame.height), (1080, 1920));
    sim.input(&t, Input::Tap { x: 0.5, y: 0.25 }, true)
        .await
        .unwrap();
    sim.input(
        &t,
        Input::Text {
            text: "a ';$(x)& b".into(),
        },
        true,
    )
    .await
    .unwrap();
    sim.input(&t, Input::Home, true).await.unwrap();
    let calls = fake.calls.lock().unwrap().clone();
    assert!(
        calls
            .iter()
            .any(|(_, a)| a == &["-avd", "Pixel_8", "-no-window", "-no-audio"])
    );
    assert!(
        calls
            .iter()
            .any(|(_, a)| a == &["-s", "emulator-5554", "shell", "input", "tap", "540", "480"])
    );
    assert!(
        calls
            .iter()
            .any(|(_, a)| a.last().is_some_and(|a| a == "'a%s'\\'';$(x)&%sb'"))
    );
    assert!(
        !calls
            .iter()
            .any(|(_, a)| a.iter().any(|a| a == "real-phone"))
    );
    sim.close(&t, true, true).await.unwrap();
    assert!(sim.state("chat").await.sessions.is_empty());
}
#[tokio::test]
async fn ios_uses_explicit_udid_waits_for_boot_and_converts_pixels_to_points() {
    let (sim, fake) = controller(true);
    let t = target(UDID);
    sim.configure("chat", true, true).await;
    sim.open(&t, false).await.unwrap();
    sim.input(&t, Input::Tap { x: 1.0, y: 1.0 }, true)
        .await
        .unwrap();
    sim.input(
        &t,
        Input::Text {
            text: "--hello world".into(),
        },
        true,
    )
    .await
    .unwrap();
    let calls = fake.calls.lock().unwrap().clone();
    assert!(
        calls
            .iter()
            .any(|(_, a)| a == &["simctl", "bootstatus", UDID, "-b"])
    );
    assert!(
        calls
            .iter()
            .any(|(_, a)| a == &["ui", "tap", "--udid", UDID, "--", "359", "639"])
    );
    assert!(sim.input(&t, Input::Back, true).await.is_err());
    assert!(
        sim.input(
            &t,
            Input::Text {
                text: "こんにちは".into()
            },
            true
        )
        .await
        .is_err()
    );
}
#[tokio::test]
async fn consent_sessions_and_cross_chat_shutdown_are_enforced() {
    let (sim, _) = controller(false);
    let t = target("android:Pixel_8");
    sim.configure("chat", true, false).await;
    assert!(sim.open(&t, true).await.is_err());
    sim.open(&t, false).await.unwrap();
    assert!(sim.screenshot(&t, true).await.is_err());
    let other = Target {
        chat_id: "other".into(),
        ..t.clone()
    };
    sim.configure("other", true, true).await;
    assert!(sim.screenshot(&other, true).await.is_err());
    sim.open(&other, true).await.unwrap();
    assert!(sim.close(&t, true, false).await.is_err());
    sim.close(&other, false, true).await.unwrap();
    sim.configure("chat", false, true).await;
    assert!(sim.screenshot(&t, false).await.is_err());
    assert!(!sim.state("chat").await.agent_access);
}
#[test]
fn malformed_catalogs_frames_and_inputs_are_rejected() {
    assert!(parse_ios(b"{}").is_err());
    assert!(png_size(b"bad").is_err());
    let mut huge = png();
    huge[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(png_size(&huge).is_err());
    for (x, y) in [
        (f64::NAN, 0.0),
        (0.0, f64::INFINITY),
        (-0.1, 0.0),
        (1.1, 0.0),
    ] {
        assert!(validate_input(&Input::Tap { x, y }).is_err());
    }
    assert!(
        validate_input(&Input::Text {
            text: "a".repeat(4097)
        })
        .is_err()
    );
    assert!(!emulator_serial("physical"));
    assert!(!valid_avd("-wipe-data"));
}
#[test]
fn ios_catalog_filters_unavailable_and_other_platforms() {
    let bytes = serde_json::to_vec(&serde_json::json!({"devices":{
        "com.apple.CoreSimulator.SimRuntime.iOS-18-0":[{"udid":UDID,"name":"iPhone","isAvailable":true,"state":"Booted"},{"udid":UDID,"isAvailable":false}],
        "com.apple.CoreSimulator.SimRuntime.tvOS-18-0":[{"udid":UDID,"isAvailable":true}]
    }})).unwrap();
    let devices = parse_ios(&bytes).unwrap();
    assert_eq!(devices.len(), 1);
    assert!(devices[0].booted);
}
#[cfg(unix)]
#[tokio::test]
async fn real_runner_bounds_output_and_deadlines() {
    let runner = SystemRunner;
    let result = runner
        .run(
            "sh",
            &["-c".into(), "sleep 2".into()],
            Duration::from_millis(10),
        )
        .await;
    assert!(result.unwrap_err().to_string().contains("timed out"));
    let result = runner
        .run(
            "sh",
            &["-c".into(), "head -c 8388610 /dev/zero".into()],
            COMMAND_TIMEOUT,
        )
        .await;
    assert!(result.unwrap_err().to_string().contains("8 MiB"));
}

#[tokio::test]
async fn missing_tools_return_setup_instead_of_starting_processes() {
    let (sim, fake) = controller(true);
    *fake.unavailable.lock().unwrap() = vec![
        sim.0.tools.xcrun.clone(),
        sim.0.tools.adb.clone(),
        sim.0.tools.emulator.clone(),
        sim.0.tools.idb.clone(),
        sim.0.tools.avdmanager.clone(),
    ];
    let state = sim.state("chat").await;
    assert!(state.devices.is_empty());
    assert!(state.requirements.iter().all(|r| !r.ready));
    assert!(state.requirements.iter().any(|r| r.message.contains("JDK")));
    assert!(!*fake.started.lock().unwrap());

    // An emulator binary and AVD alone cannot drive a device without adb.
    let (sim, fake) = controller(false);
    *fake.unavailable.lock().unwrap() = vec![sim.0.tools.adb.clone()];
    sim.configure("chat", true, true).await;
    assert!(sim.state("chat").await.devices.is_empty());
    assert!(sim.open(&target("android:Pixel_8"), true).await.is_err());
    assert!(!*fake.started.lock().unwrap());
}
#[tokio::test]
async fn reused_serial_cannot_control_a_different_avd() {
    let (sim, fake) = controller(false);
    let target = target("android:Pixel_8");
    sim.configure("chat", true, true).await;
    sim.open(&target, true).await.unwrap();
    *fake.avd_override.lock().unwrap() = Some("Fresh_AVD".into());
    assert!(
        sim.screenshot(&target, true)
            .await
            .unwrap_err()
            .to_string()
            .contains("another AVD")
    );
    assert!(sim.input(&target, Input::Home, true).await.is_err());
    assert!(sim.close(&target, true, true).await.is_err());
    sim.close(&target, false, true).await.unwrap();
}

#[tokio::test]
async fn deleted_chats_release_consent_and_do_not_block_other_viewers() {
    let (sim, _) = controller(false);
    sim.configure("chat", true, true).await;
    let first = target("android:Pixel_8");
    sim.open(&first, true).await.unwrap();
    sim.configure("other", true, true).await;
    let second = Target {
        chat_id: "other".into(),
        ..first.clone()
    };
    sim.open(&second, true).await.unwrap();
    sim.forget_chat("chat");
    assert!(sim.screenshot(&first, true).await.is_err());
    sim.close(&second, true, true).await.unwrap();
    sim.retain_chats(&Default::default());
    assert!(!sim.state("other").await.enabled);
}

#[tokio::test]
async fn closing_a_pane_during_boot_releases_the_late_session() {
    #[derive(Default)]
    struct BootLatch {
        fake: Fake,
        entered: tokio::sync::Notify,
        resume: tokio::sync::Notify,
    }
    #[async_trait]
    impl Runner for BootLatch {
        async fn run(&self, program: &str, args: &[String], timeout: Duration) -> Result<Vec<u8>> {
            if args.iter().any(|a| a == "bootstatus") {
                self.entered.notify_one();
                self.resume.notified().await;
            }
            self.fake.run(program, args, timeout).await
        }
        async fn start(&self, _: &str, _: &[String]) -> Result<()> {
            bail!("unexpected emulator spawn")
        }
    }
    let runner = Arc::new(BootLatch::default());
    let sim = Simulators::new(
        runner.clone(),
        Toolchain {
            macos: true,
            ..Default::default()
        },
    );
    sim.configure("chat", true, true).await;
    let opening = sim.clone();
    let boot = tokio::spawn(async move { opening.open(&target(UDID), false).await.unwrap() });
    runner.entered.notified().await;
    let closing = sim.clone();
    let close = tokio::spawn(async move {
        closing.close_chat("chat").await;
    });
    runner.resume.notify_one();
    boot.await.unwrap();
    close.await.unwrap();
    let state = sim.state("chat").await;
    assert!(state.sessions.is_empty());
    assert!(state.enabled);
    let opening = sim.clone();
    let boot = tokio::spawn(async move { opening.open(&target(UDID), true).await });
    runner.entered.notified().await;
    tokio::time::timeout(
        Duration::from_millis(100),
        sim.configure("chat", true, false),
    )
    .await
    .expect("revocation must not wait behind boot");
    runner.resume.notify_one();
    assert!(boot.await.unwrap().is_err());
    assert!(sim.state("chat").await.sessions.is_empty());
    sim.configure("chat", true, true).await;
    let opening = sim.clone();
    let boot = tokio::spawn(async move { opening.open(&target(UDID), false).await });
    runner.entered.notified().await;
    sim.shutdown();
    assert!(boot.await.unwrap().is_err());
    sim.configure("chat", true, true).await;
    assert!(!sim.state("chat").await.enabled);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancelling_a_bounded_command_kills_its_descendants() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("child-pid");
    let args = vec![
        "-c".into(),
        "sleep 30 & echo $! > \"$1\"; wait".into(),
        "device-test".into(),
        file.to_string_lossy().into_owned(),
    ];
    let job =
        tokio::spawn(async move { SystemRunner.run("sh", &args, Duration::from_secs(30)).await });
    let pid = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(&file) {
                if let Ok(pid) = pid.trim().parse::<u32>() {
                    break pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    job.abort();
    let _ = job.await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let state = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok();
            if state.as_ref().is_none_or(|s| {
                s.rsplit_once(')').is_some_and(|(_, tail)| {
                    matches!(tail.split_whitespace().next(), Some("Z" | "X"))
                })
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}
