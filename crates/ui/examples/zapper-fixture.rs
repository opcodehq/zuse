//! Native visual evidence for Zapper avatars and the shared issue surfaces,
//! drawn from offline data. Usage: `zapper-fixture <capture directory>`.
use gpui::{
    AnyWindowHandle, AppContext, AsyncApp, Bounds, Context, Entity, IntoElement, ParentElement,
    Render, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use zuse_protocol::presentation::zapper_mascots::{self, Mood};
use zuse_protocol::zappers::*;
use zuse_ui::*;
use zuse_viewport::zapper_avatar::{
    ZapperFace, person_avatar, prepare_image, unassigned_avatar, zapper_avatar, zapper_face,
};
use zuse_viewport::zapper_issues::{IssuePanel, ThreadState};

async fn pause(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(450))
        .await;
}

async fn capture(window: AnyWindowHandle, cx: &mut AsyncApp, path: &Path) -> anyhow::Result<()> {
    window.update(cx, |_, w, cx| {
        w.refresh();
        w.draw(cx).clear();
        w.present_if_needed();
    })?;
    pause(cx).await;
    let status = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/private-secrets-fixture/capture.py"
        ))
        .arg(std::process::id().to_string())
        .arg(path)
        .status()?;
    anyhow::ensure!(status.success(), "fixture window capture failed");
    Ok(())
}

fn issue(
    number: u32,
    title: &str,
    status: ZapperIssueStatus,
    assigned: bool,
    pr: bool,
) -> ZapperIssue {
    let now = chrono::Utc::now().timestamp_millis();
    ZapperIssue {
        id: format!("issue-{number}"),
        number,
        details: ZapperIssueDetails {
            title: title.into(),
            description: "Make the flow work end to end and verify it on staging.".into(),
            status,
            priority: if number % 3 == 0 {
                ZapperIssuePriority::High
            } else {
                ZapperIssuePriority::Normal
            },
            assigned,
            owner: Some(match status {
                ZapperIssueStatus::NeedsYou => ZapperIssueOwner::User,
                ZapperIssueStatus::InProgress | ZapperIssueStatus::InReview => {
                    ZapperIssueOwner::Thread {
                        chat_id: format!("zapper-worker-{number}"),
                    }
                }
                _ => ZapperIssueOwner::Zapper,
            }),
            done_criteria: "Works end to end on staging with a passing smoke test.".into(),
            ..Default::default()
        },
        created_at: now - 54_000_000,
        updated_at: now - i64::from(number) * 3_600_000,
        threads: vec![format!("zapper-worker-{number}")],
        pull_requests: if pr {
            vec![ZapperIssuePullRequest {
                url: format!("https://github.com/acme/app/pull/{}", 120 + number),
                title: title.into(),
            }]
        } else {
            vec![]
        },
        activity: vec![],
    }
}

fn sample_zapper(issues: Vec<ZapperIssue>, workers: usize) -> Zapper {
    Zapper {
        id: "release".into(),
        revision: 4,
        activity_revision: 0,
        config: ZapperConfig {
            name: "Release".into(),
            purpose: "Keep staging healthy and ship the cloud chat work.".into(),
            home_device_id: "local".into(),
            repositories: vec![],
            coordinator_model: ZapperModel {
                harness: "claude-code".into(),
                model: None,
            },
            worker_device_id: "local".into(),
            max_concurrent_workers: 3,
            max_followups: 3,
            avatar: Some(ZapperAvatar {
                mascot: "bolt".into(),
                color: "violet".into(),
            }),
            avatar_image: None,
        },
        status: ZapperStatus::Active,
        coordinator_chat_id: "zapper-release-0".into(),
        previous_coordinator_chats: vec![],
        workers: (1..=workers as u32)
            .map(|n| ZapperWorker {
                chat_id: format!("zapper-worker-{n}"),
                device_id: "local".into(),
                space_id: "project".into(),
                objective: [
                    "Test access mode and queued files on a new image",
                    "Send cloud chat files straight to the computer",
                    "Queue edits in the history window",
                ][(n as usize - 1) % 3]
                    .into(),
                followups: 0,
                queue_position: None,
            })
            .collect(),
        decisions: vec![],
        issues,
        folders: vec![],
    }
}

/// A stand-in for a user's uploaded photo: a sunset with a sun and hills,
/// big and wide so the crop and downscale both run.
fn sample_photo() -> String {
    let (w, h) = (1200u32, 800u32);
    let image = image::RgbaImage::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
        let sun = ((fx - 0.5).powi(2) + (fy - 0.55).powi(2)).sqrt() < 0.16;
        let hill = fy > 0.72 + 0.06 * (fx * 9.0).sin();
        let px = if hill {
            [40, 70 + (fy * 40.) as u8, 60, 255]
        } else if sun {
            [255, 214, 120, 255]
        } else {
            [
                (250. - fy * 120.) as u8,
                (120. + fy * 40.) as u8,
                (110. + fy * 110.) as u8,
                255,
            ]
        };
        image::Rgba(px)
    });
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    prepare_image(&bytes).expect("sample photo prepares")
}

struct Board {
    picture: ZapperFace,
    issues: Entity<IssuePanel>,
    overview: Entity<IssuePanel>,
    empty: Entity<IssuePanel>,
}

impl Render for Board {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let label = |text: &str| {
            div()
                .text_size(px(11.))
                .text_color(rgb(0x9a9a9a))
                .child(text.to_owned())
        };
        let mut faces = div().flex().gap(px(18.)).items_end();
        let mut sidebar = div().flex().flex_col().gap(px(6.)).w(px(190.));
        for (index, mascot) in zapper_mascots::MASCOTS.iter().enumerate() {
            let color = &zapper_mascots::COLORS[(index * 3 + 4) % zapper_mascots::COLORS.len()];
            let avatar = ZapperAvatar {
                mascot: mascot.id.into(),
                color: color.id.into(),
            };
            faces = faces.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .child(zapper_avatar(&avatar, 64.))
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap(px(6.))
                            .child(zapper_avatar(&avatar, 40.))
                            .child(zapper_avatar(&avatar, 16.)),
                    )
                    .child(label(mascot.name)),
            );
            sidebar = sidebar.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(13.))
                    .text_color(rgb(0xd8d8d8))
                    .child(zapper_avatar(&avatar, 16.))
                    .child(format!("{} Zapper", mascot.name)),
            );
        }
        let picture = &self.picture;
        faces = faces.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .child(zapper_face(picture, 64.))
                .child(
                    div()
                        .flex()
                        .items_end()
                        .gap(px(6.))
                        .child(zapper_face(picture, 40.))
                        .child(zapper_face(picture, 16.)),
                )
                .child(label("Own image")),
        );
        sidebar = sidebar.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(13.))
                .text_color(rgb(0xd8d8d8))
                .child(zapper_face(picture, 16.))
                .child("Photo Zapper"),
        );
        let header = |face: ZapperFace, name: &str| {
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .p(px(12.))
                .w(px(300.))
                .bg(rgb(0x141414))
                .border_1()
                .border_color(rgb(0x262626))
                .rounded(px(8.))
                .child(zapper_face(&face, 40.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(15.))
                                .text_color(rgb(0xf0f0f0))
                                .child(name.to_owned()),
                        )
                        .child(label("Active · 2 threads")),
                )
        };
        let headers = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(header(
                ZapperFace::mascot(ZapperAvatar {
                    mascot: "hex".into(),
                    color: "teal".into(),
                }),
                "Release",
            ))
            .child(header(picture.clone(), "Photo Zapper"))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(person_avatar("Sam", 24.))
                    .child(unassigned_avatar(16.)),
            );
        // Every mood on one mascot, at header and sidebar size.
        let mut moods = div().flex().gap(px(22.)).items_end();
        for (mood, name) in [
            (Mood::Resting, "Resting"),
            (Mood::Working, "Working"),
            (Mood::Delegating, "Threads working"),
            (Mood::NeedsYou, "Needs you"),
            (Mood::Awake, "Awake"),
        ] {
            let face = ZapperFace::mascot(ZapperAvatar {
                mascot: "bolt".into(),
                color: "violet".into(),
            })
            .with_mood(mood);
            moods = moods.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap(px(6.))
                            .child(zapper_face(&face, 64.))
                            .child(zapper_face(&face, 16.)),
                    )
                    .child(label(name)),
            );
        }
        let faces = div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(
                div()
                    .flex()
                    .gap(px(28.))
                    .items_start()
                    .child(faces)
                    .child(sidebar)
                    .child(headers),
            )
            .child(moods);
        let panel = |child: Entity<IssuePanel>| {
            div()
                .w(px(520.))
                .p(px(16.))
                .bg(rgb(0x101010))
                .border_1()
                .border_color(rgb(0x262626))
                .child(child)
        };
        div()
            .size_full()
            .bg(rgb(0x0b0b0b))
            .p(px(20.))
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(faces)
            .child(
                div()
                    .flex()
                    .gap(px(20.))
                    .items_start()
                    .child(panel(self.issues.clone()))
                    .child(panel(self.overview.clone()))
                    .child(panel(self.empty.clone())),
            )
    }
}

fn main() -> anyhow::Result<()> {
    let output = PathBuf::from(std::env::args().nth(1).expect("capture directory"));
    std::fs::create_dir_all(&output)?;
    let failure = std::sync::Arc::new(std::sync::Mutex::new(None));
    let result = failure.clone();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_base::init(cx);
            let issues = vec![
                issue(
                    1,
                    "Staging: test access mode and queued files on a new image",
                    ZapperIssueStatus::InProgress,
                    true,
                    false,
                ),
                issue(
                    2,
                    "Web app sends cloud chat files straight to the computer",
                    ZapperIssueStatus::Blocked,
                    true,
                    false,
                ),
                issue(
                    3,
                    "Pick the staging machine image to test",
                    ZapperIssueStatus::NeedsYou,
                    false,
                    false,
                ),
                issue(
                    4,
                    "Queue edits and the history window send files",
                    ZapperIssueStatus::InReview,
                    true,
                    true,
                ),
                issue(
                    5,
                    "Keep the private key out of computers and workloads",
                    ZapperIssueStatus::InReview,
                    true,
                    true,
                ),
                issue(
                    6,
                    "Remove the old upload path",
                    ZapperIssueStatus::Done,
                    true,
                    false,
                ),
            ];
            let board = sample_zapper(issues, 3);
            let states = [
                ("zapper-worker-1".to_owned(), ThreadState::Working),
                ("zapper-worker-2".to_owned(), ThreadState::NeedsYou),
            ]
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(12.), px(30.)),
                            size(px(1660.), px(900.)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| {
                        let make = |zapper: Zapper, overview: bool, cx: &mut gpui::App| {
                            let states = states.clone();
                            cx.new(|cx| {
                                let mut panel = IssuePanel::new(zapper, overview);
                                panel.thread_states(states, cx);
                                panel
                            })
                        };
                        let issues = make(board.clone(), false, cx);
                        let overview = make(board.clone(), true, cx);
                        let empty = make(sample_zapper(vec![], 0), true, cx);
                        cx.new(|_| Board {
                            picture: ZapperFace {
                                avatar: ZapperAvatar {
                                    mascot: "bolt".into(),
                                    color: "coral".into(),
                                },
                                image: Some(sample_photo().into()),
                                mood: Default::default(),
                            },
                            issues,
                            overview,
                            empty,
                        })
                    },
                )
                .unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let run: anyhow::Result<()> = async {
                    pause(cx).await;
                    capture(window.into(), cx, &output.join("zappers.png")).await?;
                    // A frame sequence of the idle loop. Each capture redraws,
                    // so frames land wherever the shared clock is.
                    if std::env::var_os("ZAPPER_FRAMES").is_some() {
                        for frame in 0..24 {
                            // A pointer circling the mascot strip, so the
                            // clip shows heads and eyes following it.
                            let turn = frame as f32 / 24. * std::f32::consts::TAU;
                            let at = gpui::point(
                                px(520. + 480. * turn.cos()),
                                px(110. + 160. * turn.sin()),
                            );
                            cx.update(|cx| {
                                zuse_viewport::zapper_avatar::fixture_point_at(Some(at), cx)
                            });
                            capture(
                                window.into(),
                                cx,
                                &output.join(format!("idle-{frame:02}.png")),
                            )
                            .await?;
                        }
                    }
                    Ok(())
                }
                .await;
                if let Err(error) = run {
                    *failure.lock().unwrap() = Some(format!("{error:#}"));
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    if let Some(error) = result.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
